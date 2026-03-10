use std::sync::Arc;

use deepagents::provider::{
    LlmEvent, LlmProvider, OpenAiChatChunk, OpenAiChatResponse, OpenAiChoice, OpenAiChunkChoice,
    OpenAiCompatibleConfig, OpenAiCompatibleProvider, OpenAiDelta, OpenAiFunctionCall,
    OpenAiFunctionCallDelta, OpenAiMessage, OpenAiToolCall, OpenAiToolCallDelta, OpenAiUsage,
    ProviderRequest, ProviderStep, build_chat_request, MockOpenAiTransport,
};

fn sample_request() -> ProviderRequest {
    ProviderRequest {
        messages: vec![
            deepagents::types::Message {
                role: "system".to_string(),
                content: "You are helpful".to_string(),
                content_blocks: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            },
            deepagents::types::Message {
                role: "assistant".to_string(),
                content: String::new(),
                content_blocks: None,
                tool_calls: Some(vec![deepagents::types::ToolCall {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({ "file_path": "README.md" }),
                }]),
                tool_call_id: None,
                name: None,
                status: None,
            },
            deepagents::types::Message {
                role: "tool".to_string(),
                content: "{\"ok\":true}".to_string(),
                content_blocks: None,
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
                name: Some("read_file".to_string()),
                status: Some("success".to_string()),
            },
        ],
        tool_specs: vec![deepagents::runtime::ToolSpec {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
        }],
        skills: Vec::new(),
        state: deepagents::state::AgentState::default(),
        last_tool_results: Vec::new(),
    }
}

#[test]
fn openai_build_chat_request_maps_messages_and_tools() {
    let req = build_chat_request("gpt-4o-mini", &sample_request(), false);

    assert_eq!(req.model, "gpt-4o-mini");
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.messages[0].role, "system");
    assert_eq!(req.messages[1].tool_calls.as_ref().unwrap()[0].id, "call_1");
    assert_eq!(req.messages[2].tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(req.tools.len(), 1);
    assert_eq!(req.tools[0].function.name, "read_file");
}

#[tokio::test]
async fn openai_chat_response_with_tool_calls_parses_to_provider_step() {
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        Arc::new(MockOpenAiTransport::for_response(OpenAiChatResponse {
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
            usage: Some(OpenAiUsage {
                prompt_tokens: Some(12),
                completion_tokens: Some(7),
                total_tokens: Some(19),
            }),
        })),
    );

    let step = provider.chat(sample_request()).await.unwrap();
    assert!(matches!(
        step,
        ProviderStep::ToolCalls { calls }
            if calls.len() == 1
                && calls[0].tool_name == "read_file"
                && calls[0].call_id.as_deref() == Some("call_1")
    ));
}

#[tokio::test]
async fn openai_stream_chat_aggregates_chunks_into_delta_and_final_step() {
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        Arc::new(MockOpenAiTransport::for_chunks(vec![
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
        ])),
    );

    let mut stream = provider.stream_chat(sample_request()).await.unwrap();
    let mut events = Vec::new();
    use tokio_stream::StreamExt;
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }

    assert!(matches!(
        &events[0],
        LlmEvent::AssistantTextDelta { text } if text == "Hel"
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        LlmEvent::AssistantTextDelta { text } if text == "lo"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        LlmEvent::ToolCallArgsDelta { tool_call_id, .. } if tool_call_id == "call_1"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        LlmEvent::Usage {
            input_tokens: Some(10),
            output_tokens: Some(5),
            total_tokens: Some(15),
        }
    )));
    assert!(matches!(
        events.last(),
        Some(LlmEvent::FinalStep {
            step: ProviderStep::ToolCalls { calls }
        }) if calls.len() == 1
            && calls[0].tool_name == "read_file"
            && calls[0].call_id.as_deref() == Some("call_1")
            && calls[0].arguments == serde_json::json!({ "file_path": "README.md" })
    ));
}
