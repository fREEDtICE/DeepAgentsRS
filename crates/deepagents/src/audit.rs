use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub timestamp_ms: i64,
    pub root: String,
    pub mode: String,
    pub command_redacted: String,
    pub decision: String,
    pub decision_code: String,
    pub decision_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent) -> anyhow::Result<()>;
}

#[derive(Clone)]
pub struct NoopAuditSink;

impl AuditSink for NoopAuditSink {
    fn record(&self, _event: AuditEvent) -> anyhow::Result<()> {
        Ok(())
    }
}
