use std::sync::Arc;

use async_trait::async_trait;
use deepagents::llm::openai_compatible::{
    MockOpenAiTransport, OpenAiChatChunk, OpenAiChatRequest, OpenAiChatResponse, OpenAiChoice,
    OpenAiChunkChoice, OpenAiCompatibleConfig, OpenAiCompatibleProvider, OpenAiCompatibleTransport,
    OpenAiContentPart, OpenAiDelta, OpenAiFunctionCall, OpenAiFunctionCallDelta,
    OpenAiJsonSchemaResponseFormat, OpenAiMessage, OpenAiMessageContent, OpenAiResponseFormat,
    OpenAiToolCall, OpenAiToolCallDelta, OpenAiToolChoice, OpenAiToolChoiceFunction, OpenAiUsage,
};
use deepagents::llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatRole, LlmEvent, LlmProvider,
    LlmProviderCapabilities, MultimodalCapabilities, MultimodalInputRoles, StructuredOutputSpec,
    ToolCall, ToolChoice, ToolSpec,
};
use deepagents::types::ContentBlock;

fn sample_request() -> ChatRequest {
    ChatRequest {
        messages: vec![
            ChatMessage {
                role: ChatRole::System,
                content: "You are helpful".to_string(),
                content_blocks: None,
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: String::new(),
                content_blocks: None,
                reasoning_content: Some("Need to inspect the file first.".to_string()),
                tool_calls: Some(vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({ "file_path": "README.md" }),
                }]),
                tool_call_id: None,
                name: None,
                status: None,
            },
            ChatMessage {
                role: ChatRole::Tool,
                content: "{\"ok\":true}".to_string(),
                content_blocks: None,
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
                name: Some("read_file".to_string()),
                status: Some("success".to_string()),
            },
        ],
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

#[derive(Clone)]
struct CaptureTransport {
    response: OpenAiChatResponse,
    last_request: Arc<std::sync::Mutex<Option<OpenAiChatRequest>>>,
}

impl CaptureTransport {
    fn new(response: OpenAiChatResponse) -> Self {
        Self {
            response,
            last_request: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn last_request(&self) -> OpenAiChatRequest {
        self.last_request
            .lock()
            .unwrap()
            .clone()
            .expect("captured request")
    }
}

#[async_trait]
impl OpenAiCompatibleTransport for CaptureTransport {
    async fn create_chat_completion(
        &self,
        _config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
    ) -> anyhow::Result<OpenAiChatResponse> {
        *self.last_request.lock().unwrap() = Some(request);
        Ok(self.response.clone())
    }

    async fn stream_chat_completion(
        &self,
        _config: &OpenAiCompatibleConfig,
        _request: OpenAiChatRequest,
    ) -> anyhow::Result<deepagents::llm::openai_compatible::OpenAiChunkStream> {
        Err(anyhow::anyhow!("streaming unsupported in CaptureTransport"))
    }
}

#[tokio::test]
async fn openai_build_chat_request_maps_messages_and_tools() {
    let transport = Arc::new(CaptureTransport::new(OpenAiChatResponse {
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
    }));
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        transport.clone(),
    );
    let sample = sample_request();
    let _ = provider.chat(sample).await.unwrap();
    let req = transport.last_request();

    assert_eq!(req.model, "gpt-4o-mini");
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.messages[0].role, "system");
    assert_eq!(
        req.messages[1].reasoning_content.as_deref(),
        Some("Need to inspect the file first.")
    );
    assert_eq!(req.messages[1].tool_calls.as_ref().unwrap()[0].id, "call_1");
    assert_eq!(req.messages[2].tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(req.tools.len(), 1);
    assert_eq!(req.tools[0].function.name, "read_file");
    assert_eq!(
        req.tools[0].function.parameters,
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string" }
            },
            "required": ["file_path"],
            "additionalProperties": false
        })
    );
    assert_eq!(req.tool_choice, None);
}

#[tokio::test]
async fn openai_build_chat_request_encodes_user_image_blocks_as_content_parts() {
    let transport = Arc::new(CaptureTransport::new(OpenAiChatResponse {
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
    }));
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        transport.clone(),
    );

    let req = ChatRequest {
        messages: vec![ChatMessage {
            role: ChatRole::User,
            content: "Describe this image.".to_string(),
            content_blocks: Some(vec![ContentBlock::image_base64("image/png", "AAEC")]),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            status: None,
        }],
        tool_specs: Vec::new(),
        tool_choice: ToolChoice::Auto,
        structured_output: None,
    };

    let _ = provider.chat(req).await.unwrap();
    let built = transport.last_request();
    assert_eq!(
        built.messages[0].content,
        Some(OpenAiMessageContent::Parts(vec![
            OpenAiContentPart::text("Describe this image."),
            OpenAiContentPart::image_url("data:image/png;base64,AAEC"),
        ]))
    );
}

#[tokio::test]
async fn openai_build_chat_request_degrades_unsupported_role_blocks_to_text() {
    let transport = Arc::new(CaptureTransport::new(OpenAiChatResponse {
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
    }));
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        transport.clone(),
    );

    let req = ChatRequest {
        messages: vec![ChatMessage {
            role: ChatRole::Tool,
            content: "(image returned as content block)".to_string(),
            content_blocks: Some(vec![ContentBlock::image_base64("image/png", "AAEC")]),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
            name: Some("read_file".to_string()),
            status: Some("success".to_string()),
        }],
        tool_specs: Vec::new(),
        tool_choice: ToolChoice::Auto,
        structured_output: None,
    };

    let _ = provider.chat(req).await.unwrap();
    let built = transport.last_request();
    assert_eq!(
        built.messages[0].content,
        Some(OpenAiMessageContent::from(
            "(image returned as content block)".to_string()
        ))
    );
}

#[test]
fn openai_provider_declares_expected_capabilities() {
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        Arc::new(MockOpenAiTransport::for_response(OpenAiChatResponse {
            choices: Vec::new(),
            usage: None,
        })),
    );

    assert_eq!(
        provider.capabilities(),
        LlmProviderCapabilities {
            supports_streaming: true,
            supports_tool_calling: true,
            reports_usage: true,
            supports_structured_output: true,
            supports_reasoning_content: true,
            multimodal: MultimodalCapabilities::image_input_output(
                MultimodalInputRoles::user_only(),
            ),
        }
    );
}

#[tokio::test]
async fn openai_build_chat_request_maps_structured_output_response_format() {
    let mut req = sample_request();
    req.structured_output = Some(StructuredOutputSpec {
        name: "final_answer".to_string(),
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string" }
            },
            "required": ["summary"],
            "additionalProperties": false
        }),
        description: Some("Structured final answer".to_string()),
        strict: true,
    });

    let transport = Arc::new(CaptureTransport::new(OpenAiChatResponse {
        choices: vec![OpenAiChoice {
            message: OpenAiMessage {
                role: "assistant".to_string(),
                content: Some("{\"summary\":\"done\"}".into()),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
            },
            finish_reason: Some("stop".to_string()),
        }],
        usage: None,
    }));
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        transport.clone(),
    );

    let _ = provider.chat(req).await.unwrap();
    let built = transport.last_request();
    assert_eq!(
        built.response_format,
        Some(OpenAiResponseFormat {
            kind: "json_schema".to_string(),
            json_schema: OpenAiJsonSchemaResponseFormat {
                name: "final_answer".to_string(),
                description: Some("Structured final answer".to_string()),
                schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "summary": { "type": "string" }
                    },
                    "required": ["summary"],
                    "additionalProperties": false
                }),
                strict: Some(true),
            },
        })
    );
}

#[tokio::test]
async fn openai_convert_tools_maps_tool_choice_variants() {
    let mut req = sample_request();

    let transport = Arc::new(CaptureTransport::new(OpenAiChatResponse {
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
    }));
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        transport.clone(),
    );

    req.tool_choice = ToolChoice::None;
    let _ = provider.chat(req.clone()).await.unwrap();
    let built = transport.last_request();
    assert_eq!(
        built.tool_choice,
        Some(OpenAiToolChoice::Mode("none".to_string()))
    );

    req.tool_choice = ToolChoice::Required;
    let _ = provider.chat(req.clone()).await.unwrap();
    let built = transport.last_request();
    assert_eq!(
        built.tool_choice,
        Some(OpenAiToolChoice::Mode("required".to_string()))
    );

    req.tool_choice = ToolChoice::Named {
        name: "read_file".to_string(),
    };
    let _ = provider.chat(req).await.unwrap();
    let built = transport.last_request();
    assert_eq!(
        built.tool_choice,
        Some(OpenAiToolChoice::Named {
            kind: "function".to_string(),
            function: OpenAiToolChoiceFunction {
                name: "read_file".to_string(),
            },
        })
    );
}

#[tokio::test]
async fn openai_convert_tools_rejects_unknown_named_tool_choice() {
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        Arc::new(MockOpenAiTransport::for_response(OpenAiChatResponse {
            choices: Vec::new(),
            usage: None,
        })),
    );
    let mut req = sample_request();
    req.tool_choice = ToolChoice::Named {
        name: "missing_tool".to_string(),
    };

    let err = provider.chat(req).await.unwrap_err();
    assert_eq!(err.to_string(), "openai_unknown_tool_choice: missing_tool");
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
                    reasoning_content: Some(
                        "Need the README contents before answering.".to_string(),
                    ),
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
    assert_eq!(step.tool_calls.len(), 1);
    assert_eq!(step.tool_calls[0].name, "read_file");
    assert_eq!(step.tool_calls[0].id, "call_1");
    assert_eq!(
        step.assistant_metadata
            .as_ref()
            .and_then(|metadata| metadata.reasoning_content.as_deref()),
        Some("Need the README contents before answering.")
    );
}

#[tokio::test]
async fn openai_chat_response_with_text_and_tool_calls_preserves_both() {
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        Arc::new(MockOpenAiTransport::for_response(OpenAiChatResponse {
            choices: vec![OpenAiChoice {
                message: OpenAiMessage {
                    role: "assistant".to_string(),
                    content: Some("Let me check.".into()),
                    reasoning_content: Some(
                        "I should inspect the file before concluding.".to_string(),
                    ),
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
        })),
    );

    let step = provider.chat(sample_request()).await.unwrap();
    assert_eq!(step.text, "Let me check.");
    assert_eq!(step.tool_calls.len(), 1);
    assert_eq!(step.tool_calls[0].name, "read_file");
    assert_eq!(step.tool_calls[0].id, "call_1");
    assert_eq!(
        step.assistant_metadata
            .as_ref()
            .and_then(|metadata| metadata.reasoning_content.as_deref()),
        Some("I should inspect the file before concluding.")
    );
}

#[tokio::test]
async fn openai_chat_response_with_multimodal_parts_preserves_image_blocks() {
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        Arc::new(MockOpenAiTransport::for_response(OpenAiChatResponse {
            choices: vec![OpenAiChoice {
                message: OpenAiMessage {
                    role: "assistant".to_string(),
                    content: Some(OpenAiMessageContent::Parts(vec![
                        OpenAiContentPart::text("This is the file preview."),
                        OpenAiContentPart::image_url("data:image/png;base64,AAEC"),
                    ])),
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        })),
    );

    let step = provider.chat(sample_request()).await.unwrap();
    assert_eq!(step.text, "This is the file preview.");
    assert_eq!(
        step.assistant_metadata
            .as_ref()
            .and_then(|metadata| metadata.content_blocks.as_ref())
            .cloned(),
        Some(vec![ContentBlock::image_base64("image/png", "AAEC")])
    );
}

#[tokio::test]
async fn openai_chat_response_with_remote_image_url_preserves_url_block() {
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        Arc::new(MockOpenAiTransport::for_response(OpenAiChatResponse {
            choices: vec![OpenAiChoice {
                message: OpenAiMessage {
                    role: "assistant".to_string(),
                    content: Some(OpenAiMessageContent::Parts(vec![
                        OpenAiContentPart::image_url("https://cdn.example.com/image.png"),
                    ])),
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        })),
    );

    let step = provider.chat(sample_request()).await.unwrap();
    assert_eq!(step.text, "(image content)");
    assert_eq!(
        step.assistant_metadata
            .as_ref()
            .and_then(|metadata| metadata.content_blocks.as_ref())
            .cloned(),
        Some(vec![ContentBlock::image_url(
            "https://cdn.example.com/image.png"
        )])
    );
}

#[tokio::test]
async fn openai_chat_response_with_image_only_parts_synthesizes_fallback_text() {
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        Arc::new(MockOpenAiTransport::for_response(OpenAiChatResponse {
            choices: vec![OpenAiChoice {
                message: OpenAiMessage {
                    role: "assistant".to_string(),
                    content: Some(OpenAiMessageContent::Parts(vec![
                        OpenAiContentPart::image_url("data:image/png;base64,AAEC"),
                    ])),
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        })),
    );

    let step = provider.chat(sample_request()).await.unwrap();
    assert_eq!(step.text, "(image content: image/png)");
    assert_eq!(
        step.assistant_metadata
            .as_ref()
            .and_then(|metadata| metadata.content_blocks.as_ref())
            .cloned(),
        Some(vec![ContentBlock::image_base64("image/png", "AAEC")])
    );
}

#[tokio::test]
async fn openai_stream_chat_aggregates_chunks_into_delta_and_final_step() {
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        Arc::new(MockOpenAiTransport::for_chunks(vec![
            OpenAiChatChunk {
                choices: vec![OpenAiChunkChoice {
                    delta: OpenAiDelta {
                        content: Some("Hel".into()),
                        reasoning_content: Some("Need to finish the sentence.".to_string()),
                        tool_calls: None,
                    },
                    finish_reason: None,
                }],
                usage: None,
            },
            OpenAiChatChunk {
                choices: vec![OpenAiChunkChoice {
                    delta: OpenAiDelta {
                        content: Some("lo".into()),
                        reasoning_content: Some("The file answer is ready.".to_string()),
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
        Some(LlmEvent::FinalResponse {
            response
        }) if matches!(
                &response.assistant_metadata,
                Some(metadata)
                    if metadata.reasoning_content.as_deref()
                        == Some("Need to finish the sentence.The file answer is ready.")
            )
            && matches!(
                response,
                ChatResponse { text, tool_calls, .. }
                    if text == "Hello"
                        && tool_calls.len() == 1
                        && tool_calls[0].name == "read_file"
                        && tool_calls[0].id == "call_1"
                        && tool_calls[0].arguments == serde_json::json!({ "file_path": "README.md" })
            )
    ));
}

#[tokio::test]
async fn openai_stream_chat_preserves_multimodal_content_in_final_step() {
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        Arc::new(MockOpenAiTransport::for_chunks(vec![OpenAiChatChunk {
            choices: vec![OpenAiChunkChoice {
                delta: OpenAiDelta {
                    content: Some(OpenAiMessageContent::Parts(vec![
                        OpenAiContentPart::image_url("data:image/png;base64,AAEC"),
                    ])),
                    reasoning_content: None,
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        }])),
    );

    let mut stream = provider.stream_chat(sample_request()).await.unwrap();
    let mut events = Vec::new();
    use tokio_stream::StreamExt;
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }

    assert!(matches!(
        events.last(),
        Some(LlmEvent::FinalResponse { response })
            if matches!(
                response,
                ChatResponse { ref text, .. } if text == "(image content: image/png)"
            ) && matches!(
                response.assistant_metadata.as_ref().and_then(|metadata| metadata.content_blocks.as_ref()),
                Some(blocks)
                    if blocks == &vec![ContentBlock::image_base64("image/png", "AAEC")]
            )
    ));
}

#[tokio::test]
async fn openai_build_chat_request_uses_configured_multimodal_role_policy() {
    let transport = Arc::new(CaptureTransport::new(OpenAiChatResponse {
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
    }));
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini")
            .with_multimodal_input_roles(MultimodalInputRoles::user_and_tool()),
        transport.clone(),
    );

    let req = ChatRequest {
        messages: vec![ChatMessage {
            role: ChatRole::Tool,
            content: "(image returned as content block)".to_string(),
            content_blocks: Some(vec![ContentBlock::image_url(
                "https://cdn.example.com/tool.png",
            )]),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
            name: Some("read_file".to_string()),
            status: Some("success".to_string()),
        }],
        tool_specs: Vec::new(),
        tool_choice: ToolChoice::Auto,
        structured_output: None,
    };

    let _ = provider.chat(req).await.unwrap();
    let built = transport.last_request();
    assert_eq!(
        built.messages[0].content,
        Some(OpenAiMessageContent::Parts(vec![
            OpenAiContentPart::text("(image returned as content block)"),
            OpenAiContentPart::image_url("https://cdn.example.com/tool.png"),
        ]))
    );
}

#[tokio::test]
async fn openai_stream_chat_emits_tool_args_delta_when_id_arrives_late() {
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new("gpt-4o-mini"),
        Arc::new(MockOpenAiTransport::for_chunks(vec![
            OpenAiChatChunk {
                choices: vec![OpenAiChunkChoice {
                    delta: OpenAiDelta {
                        content: None,
                        reasoning_content: None,
                        tool_calls: Some(vec![OpenAiToolCallDelta {
                            index: 0,
                            id: None,
                            kind: Some("function".to_string()),
                            function: Some(OpenAiFunctionCallDelta {
                                name: Some("read_file".to_string()),
                                arguments: Some("{\"file_path\":\"README".to_string()),
                            }),
                        }]),
                    },
                    finish_reason: None,
                }],
                usage: None,
            },
            OpenAiChatChunk {
                choices: vec![OpenAiChunkChoice {
                    delta: OpenAiDelta {
                        content: None,
                        reasoning_content: None,
                        tool_calls: Some(vec![OpenAiToolCallDelta {
                            index: 0,
                            id: Some("call_1".to_string()),
                            kind: Some("function".to_string()),
                            function: Some(OpenAiFunctionCallDelta {
                                name: None,
                                arguments: Some(".md\"}".to_string()),
                            }),
                        }]),
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: None,
            },
        ])),
    );

    let mut stream = provider.stream_chat(sample_request()).await.unwrap();
    let mut deltas = Vec::new();
    use tokio_stream::StreamExt;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            LlmEvent::ToolCallArgsDelta {
                tool_call_id,
                delta,
            } if tool_call_id == "call_1" => {
                deltas.push(delta);
            }
            _ => {}
        }
    }

    assert!(!deltas.is_empty());
    assert!(deltas.join("").contains("\"file_path\""));
}
