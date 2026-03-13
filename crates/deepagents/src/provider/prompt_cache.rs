use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::provider::protocol::AgentProviderRequest;
use crate::runtime::stable_hash::stable_json_sha256_hex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPromptCacheStrategy {
    None,
    StablePrefix,
    CacheControl,
    ContextCache,
    CommonPrefix,
    KvReuse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPromptCacheStatus {
    Applied,
    Hit,
    Miss,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPromptCacheSource {
    Local,
    Provider,
    Hybrid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderPromptCacheHandle {
    pub payload: Value,
}

impl ProviderPromptCacheHandle {
    pub fn hash(&self) -> String {
        stable_json_sha256_hex(&self.payload)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderPromptCacheHint {
    pub strategy: ProviderPromptCacheStrategy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<ProviderPromptCacheHandle>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderPromptCacheObservation {
    pub cache_source: ProviderPromptCacheSource,
    pub provider_strategy: ProviderPromptCacheStrategy,
    pub provider_cache_status: ProviderPromptCacheStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_handle_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_handle: Option<ProviderPromptCacheHandle>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptCachePlan {
    pub l0_view: Value,
    pub l1_view: Value,
    pub l2_view: Value,
    pub l0_hash: String,
    pub l1_hash: String,
    pub l2_hash: String,
    pub provider_strategy: ProviderPromptCacheStrategy,
}

impl PromptCachePlan {
    pub fn new(
        l0_view: Value,
        l1_view: Value,
        l2_view: Value,
        provider_strategy: ProviderPromptCacheStrategy,
    ) -> Self {
        let l0_hash = stable_json_sha256_hex(&l0_view);
        let l1_hash = stable_json_sha256_hex(&l1_view);
        let l2_hash = stable_json_sha256_hex(&l2_view);
        Self {
            l0_view,
            l1_view,
            l2_view,
            l0_hash,
            l1_hash,
            l2_hash,
            provider_strategy,
        }
    }

    pub fn from_agent_request(req: &AgentProviderRequest) -> Self {
        let l0_view = serde_json::json!({
            "tool_choice": req.tool_choice,
            "structured_output": req.structured_output,
        });
        let prefix_messages = req
            .messages
            .iter()
            .take_while(|m| m.role == "system" || m.role == "developer")
            .cloned()
            .collect::<Vec<_>>();
        let l1_view = serde_json::json!({
            "prefix_messages": prefix_messages,
            "tool_specs": req.tool_specs,
        });
        let summarization_event = req.state.extra.get("_summarization_event").cloned();
        let l2_view = serde_json::json!({
            "messages": req.messages,
            "summarization_event": summarization_event,
        });
        Self::new(l0_view, l1_view, l2_view, ProviderPromptCacheStrategy::None)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptPrefixArtifact {
    pub l1_hash: String,
    pub strategy: ProviderPromptCacheStrategy,
    pub created_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_handle: Option<ProviderPromptCacheHandle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_handle_hash: Option<String>,
}

impl PromptPrefixArtifact {
    pub fn new(
        l1_hash: String,
        strategy: ProviderPromptCacheStrategy,
        provider_handle: Option<ProviderPromptCacheHandle>,
    ) -> Self {
        let created_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let provider_handle_hash = provider_handle.as_ref().map(|h| h.hash());
        Self {
            l1_hash,
            strategy,
            created_at_ms,
            provider_handle,
            provider_handle_hash,
        }
    }

    pub fn hint(&self) -> ProviderPromptCacheHint {
        ProviderPromptCacheHint {
            strategy: self.strategy,
            handle: self.provider_handle.clone(),
        }
    }
}
