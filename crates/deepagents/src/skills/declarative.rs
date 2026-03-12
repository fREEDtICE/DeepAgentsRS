use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::provider::AgentToolCall;
use crate::skills::protocol::{SkillCall, SkillError, SkillPlugin, SkillSpec};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclarativeSkillsManifest {
    #[serde(default)]
    pub skills: Vec<DeclarativeSkill>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclarativeSkill {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tool_calls: Vec<DeclarativeToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclarativeToolCall {
    pub tool_name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

#[derive(Clone)]
pub struct DeclarativeSkillPlugin {
    manifest: Arc<DeclarativeSkillsManifest>,
}

impl DeclarativeSkillPlugin {
    pub fn load_from_file(path: &str) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path)?;
        let manifest: DeclarativeSkillsManifest = serde_json::from_slice(&bytes)?;
        Ok(Self {
            manifest: Arc::new(manifest),
        })
    }
}

#[async_trait]
impl SkillPlugin for DeclarativeSkillPlugin {
    fn list_skills(&self) -> Vec<SkillSpec> {
        self.manifest
            .skills
            .iter()
            .map(|s| SkillSpec {
                name: s.name.clone(),
                description: s.description.clone(),
            })
            .collect()
    }

    async fn call(&self, call: SkillCall) -> Result<Vec<AgentToolCall>, SkillError> {
        let skill = self
            .manifest
            .skills
            .iter()
            .find(|s| s.name == call.name)
            .cloned()
            .ok_or_else(|| SkillError {
                code: "skill_not_found".to_string(),
                message: format!("skill not found: {}", call.name),
            })?;

        let mut out = Vec::new();
        for tc in skill.tool_calls {
            let args = merge_args(tc.arguments, &call.input);
            out.push(AgentToolCall {
                tool_name: tc.tool_name,
                arguments: args,
                call_id: call.call_id.clone(),
            });
        }
        Ok(out)
    }
}

fn merge_args(base: serde_json::Value, overlay: &serde_json::Value) -> serde_json::Value {
    let Some(overlay_map) = overlay.as_object() else {
        return base;
    };
    let mut out = match base {
        serde_json::Value::Object(m) => m,
        other => return other,
    };
    for (k, v) in overlay_map {
        out.insert(k.clone(), v.clone());
    }
    serde_json::Value::Object(out)
}
