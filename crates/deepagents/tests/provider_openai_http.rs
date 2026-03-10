use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::post;
use axum::{Json, Router};
use deepagents::provider::{
    LlmEvent, LlmProvider, OpenAiChatChunk, OpenAiChatResponse, OpenAiChoice, OpenAiChunkChoice,
    OpenAiCompatibleConfig, OpenAiCompatibleProvider, OpenAiDelta, OpenAiFunctionCall,
    OpenAiFunctionCallDelta, OpenAiMessage, OpenAiToolCall, OpenAiToolCallDelta, OpenAiUsage,
    ProviderRequest, ReqwestOpenAiTransport,
};
use tokio_stream::StreamExt;

#[derive(Clone, Default)]
struct CaptureState {
    auth_header: Arc<tokio::sync::Mutex<Option<String>>>,
    last_body: Arc<tokio::sync::Mutex<Option<serde_json::Value>>>,
}

fn sample_request() -> ProviderRequest {
    ProviderRequest {
        messages: vec![deepagents::types::Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            content_blocks: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }],
        tool_specs: vec![deepagents::runtime::ToolSpec {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string" }
                },
                "required": ["file_path"],
                "additionalProperties": false
            }),
        }],
        skills: Vec::new(),
        state: deepagents::state::AgentState::default(),
        last_tool_results: Vec::new(),
    }
}

#[tokio::test]
async fn reqwest_transport_posts_json_and_parses_chat_response() {
    let state = CaptureState::default();
    let app = Router::new()
        .route("/chat/completions", post(chat_handler))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini")
            .with_base_url(format!("http://{}", addr))
            .with_api_key("test-key"),
        Arc::new(ReqwestOpenAiTransport::new()),
    );

    let step = provider.chat(sample_request()).await.unwrap();
    assert!(matches!(
        step,
        deepagents::provider::ProviderStep::ToolCalls { calls }
            if calls.len() == 1 && calls[0].tool_name == "read_file"
    ));

    let auth = state.auth_header.lock().await.clone();
    assert_eq!(auth.as_deref(), Some("Bearer test-key"));
    let body = state.last_body.lock().await.clone().unwrap();
    assert_eq!(body["model"], "gpt-4o-mini");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(
        body["tools"][0]["function"]["parameters"]["required"][0],
        "file_path"
    );
}

#[tokio::test]
async fn reqwest_transport_parses_streaming_sse_chunks() {
    let state = CaptureState::default();
    let app = Router::new()
        .route("/chat/completions", post(stream_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini").with_base_url(format!("http://{}", addr)),
        Arc::new(ReqwestOpenAiTransport::new()),
    );

    let mut stream = provider.stream_chat(sample_request()).await.unwrap();
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }

    assert!(events.iter().any(|event| matches!(
        event,
        LlmEvent::AssistantTextDelta { text } if text == "Hel"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        LlmEvent::ToolCallArgsDelta { tool_call_id, .. } if tool_call_id == "call_1"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        LlmEvent::Usage {
            total_tokens: Some(15),
            ..
        }
    )));
}

async fn chat_handler(
    State(state): State<CaptureState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<OpenAiChatResponse>) {
    *state.auth_header.lock().await = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    *state.last_body.lock().await = Some(body);

    (
        StatusCode::OK,
        Json(OpenAiChatResponse {
            choices: vec![OpenAiChoice {
                message: OpenAiMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_calls: Some(vec![OpenAiToolCall {
                        id: "call_1".to_string(),
                        kind: "function".to_string(),
                        function: OpenAiFunctionCall {
                            name: "read_file".to_string(),
                            arguments: "{\"file_path\":\"README.md\"}".to_string(),
                        },
                    }]),
                    tool_call_id: None,
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
            usage: None,
        }),
    )
}

async fn stream_handler(
    State(state): State<CaptureState>,
    Json(body): Json<serde_json::Value>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    *state.last_body.lock().await = Some(body);

    let chunks = vec![
        OpenAiChatChunk {
            choices: vec![OpenAiChunkChoice {
                delta: OpenAiDelta {
                    content: Some("Hel".to_string()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        },
        OpenAiChatChunk {
            choices: vec![OpenAiChunkChoice {
                delta: OpenAiDelta {
                    content: Some("lo".to_string()),
                    tool_calls: Some(vec![OpenAiToolCallDelta {
                        index: 0,
                        id: Some("call_1".to_string()),
                        kind: Some("function".to_string()),
                        function: Some(OpenAiFunctionCallDelta {
                            name: Some("read_file".to_string()),
                            arguments: Some("{\"file_path\":\"README.md\"}".to_string()),
                        }),
                    }]),
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(OpenAiUsage {
                prompt_tokens: Some(10),
                completion_tokens: Some(5),
                total_tokens: Some(15),
            }),
        },
    ];

    let stream = tokio_stream::iter(
        chunks
            .into_iter()
            .map(|chunk| Ok(Event::default().data(serde_json::to_string(&chunk).unwrap())))
            .chain(std::iter::once(Ok(Event::default().data("[DONE]")))),
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}
