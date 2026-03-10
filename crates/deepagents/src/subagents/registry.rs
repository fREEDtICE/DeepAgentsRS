use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::subagents::protocol::{CompiledSubAgent, SubAgentInfo, SubAgentRegistry};

#[derive(Default)]
pub struct InMemorySubAgentRegistry {
    agents: RwLock<HashMap<String, Arc<dyn CompiledSubAgent>>>,
}

impl InMemorySubAgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
        }
    }
}

impl SubAgentRegistry for InMemorySubAgentRegistry {
    fn register(&self, agent: Arc<dyn CompiledSubAgent>) -> anyhow::Result<()> {
        let key = agent.subagent_type().to_string();
        let mut map = self
            .agents
            .write()
            .map_err(|_| anyhow::anyhow!("registry_locked"))?;
        if map.contains_key(&key) {
            return Err(anyhow::anyhow!("subagent_already_registered: {}", key));
        }
        map.insert(key, agent);
        Ok(())
    }

    fn resolve(&self, subagent_type: &str) -> Option<Arc<dyn CompiledSubAgent>> {
        let map = self.agents.read().ok()?;
        map.get(subagent_type).cloned()
    }

    fn list(&self) -> Vec<SubAgentInfo> {
        let map = match self.agents.read() {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::new();
        for (k, v) in map.iter() {
            out.push(SubAgentInfo {
                subagent_type: k.clone(),
                description: v.description().to_string(),
            });
        }
        out.sort_by(|a, b| a.subagent_type.cmp(&b.subagent_type));
        out
    }
}
