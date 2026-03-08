use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::provider::protocol::{Provider, ProviderError, ProviderRequest, ProviderStep, ProviderToolCall};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MockStep {
    FinalText { text: String },
    FinalFromLastToolFirstLine { prefix: Option<String> },
    ToolCalls { calls: Vec<ProviderToolCall> },
    SkillCall {
        name: String,
        #[serde(default)]
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
    },
    Error { code: String, message: String },
    DelayMs { ms: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MockScript {
    #[serde(default)]
    pub steps: Vec<MockStep>,
}

#[derive(Clone)]
pub struct MockProvider {
    script: Arc<MockScript>,
    omit_call_ids: bool,
}

impl MockProvider {
    pub fn from_script(script: MockScript) -> Self {
        Self {
            script: Arc::new(script),
            omit_call_ids: false,
        }
    }

    pub fn from_script_without_call_ids(script: MockScript) -> Self {
        Self {
            script: Arc::new(script),
            omit_call_ids: true,
        }
    }

    pub fn load_from_file(path: &str) -> anyhow::Result<MockScript> {
        let bytes = std::fs::read(path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn step(&self, req: ProviderRequest) -> anyhow::Result<ProviderStep> {
        let idx = req
            .last_tool_results
            .len()
            .min(self.script.steps.len());
        let step = self
            .script
            .steps
            .get(idx)
            .cloned()
            .unwrap_or(MockStep::FinalText {
                text: String::new(),
            });

        match step {
            MockStep::DelayMs { ms } => {
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                Ok(ProviderStep::FinalText {
                    text: String::new(),
                })
            }
            MockStep::FinalText { text } => Ok(ProviderStep::FinalText { text }),
            MockStep::FinalFromLastToolFirstLine { prefix } => {
                let mut out = prefix.unwrap_or_default();
                if let Some(last) = req.last_tool_results.last() {
                    if let Some(line) = extract_first_line(&last.output) {
                        out.push_str(&line);
                    }
                }
                Ok(ProviderStep::FinalText { text: out })
            }
            MockStep::ToolCalls { mut calls } => {
                if self.omit_call_ids {
                    for c in calls.iter_mut() {
                        c.call_id = None;
                    }
                }
                Ok(ProviderStep::ToolCalls { calls })
            }
            MockStep::SkillCall { name, input, call_id } => Ok(ProviderStep::SkillCall { name, input, call_id }),
            MockStep::Error { code, message } => Ok(ProviderStep::Error {
                error: ProviderError { code, message },
            }),
        }
    }
}

fn extract_first_line(v: &serde_json::Value) -> Option<String> {
    let content = v.get("content").and_then(|c| c.as_str())?;
    let line = content.lines().next()?;
    if let Some((_, rest)) = line.split_once('→') {
        return Some(rest.to_string());
    }
    Some(line.to_string())
}
