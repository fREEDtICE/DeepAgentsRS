use std::collections::BTreeSet;

use serde::Deserialize;

use crate::provider::protocol::{ProviderRequest, ProviderStep, ProviderToolCall, ToolChoice};
use crate::runtime::ToolSpec;
use crate::types::Message;

const TOOL_CALL_TAG_OPEN: &str = "<tool_call>";
const TOOL_CALL_TAG_CLOSE: &str = "</tool_call>";

#[derive(Debug, Clone)]
pub(crate) struct PromptGuidedConfig {
    tool_choice: ToolChoice,
    tool_specs: Vec<ToolSpec>,
}

#[derive(Debug, Deserialize)]
struct PromptGuidedEnvelope {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<ProviderToolCall>,
}

impl PromptGuidedConfig {
    pub(crate) fn new(tool_choice: ToolChoice, tool_specs: Vec<ToolSpec>) -> Self {
        Self {
            tool_choice,
            tool_specs,
        }
    }

    pub(crate) fn prepare_request(
        &self,
        mut req: ProviderRequest,
        extra_instructions: &str,
    ) -> ProviderRequest {
        let system_message =
            render_system_message(&self.tool_choice, &self.tool_specs, extra_instructions);
        let idx = req
            .messages
            .iter()
            .take_while(|message| message.role == "system")
            .count();
        req.messages.insert(
            idx,
            Message {
                role: "system".to_string(),
                content: system_message,
                content_blocks: None,
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                status: None,
            },
        );
        req
    }

    pub(crate) fn parse_step(&self, step: ProviderStep) -> anyhow::Result<ProviderStep> {
        match step {
            ProviderStep::FinalText { text } => self.parse_text_step(text, true),
            ProviderStep::AssistantMessage { text } => self.parse_text_step(text, false),
            ProviderStep::AssistantMessageWithToolCalls { text, calls } => {
                self.validate_tool_calls(&calls)?;
                Ok(ProviderStep::AssistantMessageWithToolCalls { text, calls })
            }
            ProviderStep::ToolCalls { calls } => {
                self.validate_tool_calls(&calls)?;
                Ok(ProviderStep::ToolCalls { calls })
            }
            other => Ok(other),
        }
    }

    fn parse_text_step(&self, text: String, final_text: bool) -> anyhow::Result<ProviderStep> {
        let trimmed = text.trim();
        let Some(payload) = extract_tool_payload(trimmed) else {
            self.ensure_plain_text_allowed()?;
            return Ok(if final_text {
                ProviderStep::FinalText { text }
            } else {
                ProviderStep::AssistantMessage { text }
            });
        };

        let envelope: PromptGuidedEnvelope = serde_json::from_str(payload)
            .map_err(|e| anyhow::anyhow!("prompt_guided_invalid_response_json: {e}"))?;
        if envelope.content.trim().is_empty() && envelope.tool_calls.is_empty() {
            anyhow::bail!("prompt_guided_empty_response");
        }

        self.validate_tool_calls(&envelope.tool_calls)?;
        if envelope.tool_calls.is_empty() {
            self.ensure_plain_text_allowed()?;
            return Ok(if final_text {
                ProviderStep::FinalText {
                    text: envelope.content,
                }
            } else {
                ProviderStep::AssistantMessage {
                    text: envelope.content,
                }
            });
        }

        if envelope.content.trim().is_empty() {
            return Ok(ProviderStep::ToolCalls {
                calls: envelope.tool_calls,
            });
        }

        Ok(ProviderStep::AssistantMessageWithToolCalls {
            text: envelope.content,
            calls: envelope.tool_calls,
        })
    }

    fn ensure_plain_text_allowed(&self) -> anyhow::Result<()> {
        match &self.tool_choice {
            ToolChoice::Auto | ToolChoice::None => Ok(()),
            ToolChoice::Required => Err(anyhow::anyhow!("prompt_guided_tool_call_required")),
            ToolChoice::Named { name } => {
                Err(anyhow::anyhow!("prompt_guided_named_tool_required: {name}"))
            }
        }
    }

    fn validate_tool_calls(&self, calls: &[ProviderToolCall]) -> anyhow::Result<()> {
        match &self.tool_choice {
            ToolChoice::Auto | ToolChoice::None => {}
            ToolChoice::Required => {
                if calls.is_empty() {
                    anyhow::bail!("prompt_guided_tool_call_required");
                }
            }
            ToolChoice::Named { name } => {
                if calls.is_empty() {
                    anyhow::bail!("prompt_guided_named_tool_required: {name}");
                }
                for call in calls {
                    if call.tool_name != *name {
                        anyhow::bail!(
                            "prompt_guided_named_tool_mismatch: expected {name}, got {}",
                            call.tool_name
                        );
                    }
                }
            }
        }

        let known_tools = self
            .tool_specs
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<BTreeSet<_>>();
        for call in calls {
            if !known_tools.contains(call.tool_name.as_str()) {
                anyhow::bail!("prompt_guided_unknown_tool: {}", call.tool_name);
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_prompt_guided_tool_choice(
    tool_choice: &ToolChoice,
    tool_specs: &[ToolSpec],
) -> anyhow::Result<()> {
    match tool_choice {
        ToolChoice::Auto | ToolChoice::None => Ok(()),
        ToolChoice::Required => {
            if tool_specs.is_empty() {
                anyhow::bail!("prompt_guided_tool_choice_without_tools");
            }
            Ok(())
        }
        ToolChoice::Named { name } => {
            if tool_specs.iter().all(|tool| tool.name != *name) {
                anyhow::bail!("prompt_guided_unknown_tool_choice: {name}");
            }
            Ok(())
        }
    }
}

fn extract_tool_payload(text: &str) -> Option<&str> {
    let stripped = text.trim();
    if !stripped.starts_with(TOOL_CALL_TAG_OPEN) || !stripped.ends_with(TOOL_CALL_TAG_CLOSE) {
        return None;
    }

    let payload = stripped
        .strip_prefix(TOOL_CALL_TAG_OPEN)?
        .strip_suffix(TOOL_CALL_TAG_CLOSE)?
        .trim();
    if payload.is_empty() {
        None
    } else {
        Some(payload)
    }
}

fn render_system_message(
    tool_choice: &ToolChoice,
    tool_specs: &[ToolSpec],
    extra_instructions: &str,
) -> String {
    let mut sections = vec!["Tool calling fallback contract:".to_string()];
    if !extra_instructions.trim().is_empty() {
        sections.push(extra_instructions.trim().to_string());
    }
    sections.push(format!(
        "If you want to call tools, reply with nothing except:\n{}\n{{\"content\":\"optional assistant text\",\"tool_calls\":[{{\"name\":\"tool_name\",\"arguments\":{{}},\"id\":\"optional-call-id\"}}]}}\n{}\nDo not wrap this payload in markdown fences.",
        TOOL_CALL_TAG_OPEN,
        TOOL_CALL_TAG_CLOSE,
    ));
    sections.push(render_tool_choice_rules(tool_choice));
    sections.push(render_tool_catalog(tool_specs));
    sections.join("\n\n")
}

fn render_tool_choice_rules(tool_choice: &ToolChoice) -> String {
    match tool_choice {
        ToolChoice::Auto => "You may either answer normally with plain text or emit the tagged JSON payload when a tool is needed.".to_string(),
        ToolChoice::None => "Tools are disabled for this request. Reply with plain assistant text only.".to_string(),
        ToolChoice::Required => "You must emit the tagged JSON payload with at least one tool call. Do not answer with plain text only.".to_string(),
        ToolChoice::Named { name } => format!(
            "You must emit the tagged JSON payload with at least one tool call, and every tool call name must be `{name}`."
        ),
    }
}

fn render_tool_catalog(tool_specs: &[ToolSpec]) -> String {
    let mut lines = vec!["Available tools:".to_string()];
    for tool in tool_specs {
        let schema = serde_json::to_string(&tool.input_schema)
            .unwrap_or_else(|_| "{\"type\":\"object\"}".to_string());
        lines.push(format!(
            "- {}: {}\n  input_schema: {}",
            tool.name, tool.description, schema
        ));
    }
    lines.join("\n")
}
