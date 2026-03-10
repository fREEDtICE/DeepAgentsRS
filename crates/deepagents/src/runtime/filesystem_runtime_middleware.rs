use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::runtime::RuntimeMiddleware;
use crate::state::AgentState;
use crate::types::Message;

pub const LARGE_TOOL_RESULT_OFFLOAD_OPTIONS_KEY: &str = "_large_tool_result_offload_options";

#[derive(Debug, Clone)]
pub struct FilesystemRuntimeOptions {
    pub enabled: bool,
    pub tool_output_char_threshold: usize,
    pub large_result_prefix: String,
    pub excluded_tools: Vec<String>,
    pub preview_max_lines: usize,
}

impl Default for FilesystemRuntimeOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            tool_output_char_threshold: 20_000,
            large_result_prefix: "/large_tool_results".to_string(),
            excluded_tools: vec![
                "ls".to_string(),
                "glob".to_string(),
                "grep".to_string(),
                "read_file".to_string(),
                "edit_file".to_string(),
                "write_file".to_string(),
            ],
            preview_max_lines: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LargeToolResultOffloadOptions {
    pub enabled: bool,
    pub threshold_chars: usize,
    pub prefix: String,
    pub excluded_tools: Vec<String>,
    pub preview_max_lines: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FilesystemRuntimeEvent {
    pub enabled: bool,
    pub threshold_chars: usize,
    pub candidates: usize,
    pub max_candidate_chars: usize,
    pub total_candidate_chars: usize,
    pub offload_performed: bool,
    pub large_result_prefix: String,
}

#[derive(Debug, Clone)]
pub struct FilesystemRuntimeMiddleware {
    options: FilesystemRuntimeOptions,
}

impl FilesystemRuntimeMiddleware {
    pub fn new(options: FilesystemRuntimeOptions) -> Self {
        Self { options }
    }
}

#[async_trait::async_trait]
impl RuntimeMiddleware for FilesystemRuntimeMiddleware {
    async fn before_run(
        &self,
        messages: Vec<Message>,
        state: &mut AgentState,
    ) -> Result<Vec<Message>> {
        state.extra.insert(
            LARGE_TOOL_RESULT_OFFLOAD_OPTIONS_KEY.to_string(),
            serde_json::to_value(LargeToolResultOffloadOptions {
                enabled: self.options.enabled,
                threshold_chars: self.options.tool_output_char_threshold,
                prefix: self.options.large_result_prefix.clone(),
                excluded_tools: self.options.excluded_tools.clone(),
                preview_max_lines: self.options.preview_max_lines,
            })?,
        );
        Ok(messages)
    }

    async fn before_provider_step(
        &self,
        messages: Vec<Message>,
        state: &mut AgentState,
    ) -> Result<Vec<Message>> {
        let mut candidates = 0usize;
        let mut max_candidate_chars = 0usize;
        let mut total_candidate_chars = 0usize;
        for m in messages.iter().filter(|m| m.role == "tool") {
            let n = m.content.chars().count();
            if n >= self.options.tool_output_char_threshold {
                candidates += 1;
                max_candidate_chars = max_candidate_chars.max(n);
                total_candidate_chars = total_candidate_chars.saturating_add(n);
            }
        }

        let event = FilesystemRuntimeEvent {
            enabled: self.options.enabled,
            threshold_chars: self.options.tool_output_char_threshold,
            candidates,
            max_candidate_chars,
            total_candidate_chars,
            offload_performed: false,
            large_result_prefix: self.options.large_result_prefix.clone(),
        };
        state.extra.insert(
            "_filesystem_runtime_event".to_string(),
            serde_json::to_value(event)?,
        );
        Ok(messages)
    }
}
