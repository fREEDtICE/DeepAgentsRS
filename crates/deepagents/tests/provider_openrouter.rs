use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::post;
use axum::{Json, Router};
use deepagents::llm::openai_compatible::{
    OpenAiChatChunk, OpenAiChatRequest, OpenAiChatResponse, OpenAiChoice, OpenAiChunkChoice,
    OpenAiCompatibleConfig, OpenAiCompatibleTransport, OpenAiDelta, OpenAiFunctionCall,
    OpenAiFunctionCallDelta, OpenAiMessage, OpenAiMessageContent, OpenAiToolCall,
    OpenAiToolCallDelta, OpenAiToolChoice, OpenAiUsage,
};
use deepagents::llm::{
    ChatMessage, ChatRequest, LlmEvent, LlmProvider, OpenRouterConfig, OpenRouterProvider,
    ToolChoice, ToolSpec,
};
use tokio_stream::StreamExt;

#[derive(Clone, Default)]
struct CaptureState {
    auth_header: Arc<tokio::sync::Mutex<Option<String>>>,
    referer_header: Arc<tokio::sync::Mutex<Option<String>>>,
    title_header: Arc<tokio::sync::Mutex<Option<String>>>,
    last_body: Arc<tokio::sync::Mutex<Option<serde_json::Value>>>,
}

#[derive(Clone)]
struct CaptureTransport {
    response: OpenAiChatResponse,
    chunks: Vec<OpenAiChatChunk>,
    last_chat_request: Arc<std::sync::Mutex<Option<OpenAiChatRequest>>>,
    last_stream_request: Arc<std::sync::Mutex<Option<OpenAiChatRequest>>>,
}

impl CaptureTransport {
    fn new(response: OpenAiChatResponse, chunks: Vec<OpenAiChatChunk>) -> Self {
        Self {
            response,
            chunks,
            last_chat_request: Arc::new(std::sync::Mutex::new(None)),
            last_stream_request: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn last_chat_request(&self) -> OpenAiChatRequest {
        self.last_chat_request
            .lock()
            .unwrap()
            .clone()
            .expect("captured chat request")
    }

    fn last_stream_request(&self) -> OpenAiChatRequest {
        self.last_stream_request
            .lock()
            .unwrap()
            .clone()
            .expect("captured stream request")
    }
}

#[async_trait]
impl OpenAiCompatibleTransport for CaptureTransport {
    async fn create_chat_completion(
        &self,
        _config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
    ) -> anyhow::Result<OpenAiChatResponse> {
        *self.last_chat_request.lock().unwrap() = Some(request);
        Ok(self.response.clone())
    }

    async fn stream_chat_completion(
        &self,
        _config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
    ) -> anyhow::Result<deepagents::llm::openai_compatible::OpenAiChunkStream> {
        *self.last_stream_request.lock().unwrap() = Some(request);
        Ok(Box::pin(tokio_stream::iter(
            self.chunks.clone().into_iter().map(Ok::<_, anyhow::Error>),
        )))
    }
}

fn sample_request() -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage::user("hello")],
        tool_specs: vec![ToolSpec {
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
        tool_choice: ToolChoice::Auto,
        structured_output: None,
    }
}

#[tokio::test]
async fn openrouter_posts_headers_and_parses_chat_response() {
    let state = CaptureState::default();
    let app = Router::new()
        .route("/chat/completions", post(chat_handler))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let provider = OpenRouterProvider::new(
        OpenRouterConfig::new("openai/gpt-4o-mini")
            .with_base_url(format!("http://{}", addr))
            .with_api_key("test-key")
            .with_site_url("https://example.com/app")
            .with_app_name("deepagents-test"),
    );

    let response = provider.chat(sample_request()).await.unwrap();
    assert!(matches!(
        response.tool_calls.as_slice(),
        [call] if call.name == "read_file"
    ));

    assert_eq!(
        state.auth_header.lock().await.as_deref(),
        Some("Bearer test-key")
    );
    assert_eq!(
        state.referer_header.lock().await.as_deref(),
        Some("https://example.com/app")
    );
    assert_eq!(
        state.title_header.lock().await.as_deref(),
        Some("deepagents-test")
    );
    let body = state.last_body.lock().await.clone().unwrap();
    assert_eq!(body["model"], "openai/gpt-4o-mini");
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["stream"], false);
}

#[tokio::test]
async fn openrouter_stream_chat_parses_sse_chunks() {
    let state = CaptureState::default();
    let app = Router::new()
        .route("/chat/completions", post(stream_handler))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let provider = OpenRouterProvider::new(
        OpenRouterConfig::new("anthropic/claude-3.5-sonnet")
            .with_base_url(format!("http://{}", addr))
            .with_api_key("test-key")
            .with_site_url("https://example.com/stream")
            .with_app_name("deepagents-stream-test"),
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

    assert_eq!(
        state.referer_header.lock().await.as_deref(),
        Some("https://example.com/stream")
    );
    assert_eq!(
        state.title_header.lock().await.as_deref(),
        Some("deepagents-stream-test")
    );
    let body = state.last_body.lock().await.clone().unwrap();
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["stream"], true);
}

#[tokio::test]
async fn openrouter_with_transport_applies_zeroclaw_request_defaults() {
    let transport = Arc::new(CaptureTransport::new(
        OpenAiChatResponse {
            choices: vec![OpenAiChoice {
                message: OpenAiMessage {
                    role: "assistant".to_string(),
                    content: Some("done".into()),
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        },
        vec![OpenAiChatChunk {
            choices: vec![OpenAiChunkChoice {
                delta: OpenAiDelta {
                    content: Some("done".into()),
                    reasoning_content: None,
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        }],
    ));
    let provider = OpenRouterProvider::with_transport(
        OpenRouterConfig::new("openai/gpt-4o-mini"),
        transport.clone(),
    );

    let _ = provider.chat(sample_request()).await.unwrap();
    let chat_request = transport.last_chat_request();
    assert_eq!(
        chat_request.tool_choice,
        Some(OpenAiToolChoice::Mode("auto".to_string()))
    );
    assert_eq!(chat_request.stream, Some(false));

    let mut stream = provider.stream_chat(sample_request()).await.unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let stream_request = transport.last_stream_request();
    assert_eq!(
        stream_request.tool_choice,
        Some(OpenAiToolChoice::Mode("auto".to_string()))
    );
    assert_eq!(stream_request.stream, Some(true));
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
    *state.referer_header.lock().await = headers
        .get("http-referer")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    *state.title_header.lock().await = headers
        .get("x-title")
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
                    reasoning_content: None,
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
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    *state.referer_header.lock().await = headers
        .get("http-referer")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    *state.title_header.lock().await = headers
        .get("x-title")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    *state.last_body.lock().await = Some(body);

    let chunks = vec![
        OpenAiChatChunk {
            choices: vec![OpenAiChunkChoice {
                delta: OpenAiDelta {
                    content: Some(OpenAiMessageContent::from("Hel")),
                    reasoning_content: None,
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        },
        OpenAiChatChunk {
            choices: vec![OpenAiChunkChoice {
                delta: OpenAiDelta {
                    content: Some(OpenAiMessageContent::from("lo")),
                    reasoning_content: None,
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
