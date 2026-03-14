use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryActorInput {
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub workspace_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedMemoryActor {
    pub user_id: String,
    pub thread_id: String,
    pub workspace_ids: BTreeSet<String>,
}

pub trait MemoryIdentityResolver: Send + Sync {
    fn resolve_actor(&self, input: &MemoryActorInput) -> ResolvedMemoryActor;
}

#[derive(Debug, Clone)]
pub struct LocalIdentityResolver;

impl LocalIdentityResolver {
    const COMPAT_USER_ID: &'static str = "__user__";
    const COMPAT_THREAD_ID: &'static str = "__thread__";
    const COMPAT_WORKSPACE_ID: &'static str = "__compat_workspace__";

    pub fn new(root: impl Into<String>) -> Self {
        let _ = root.into();
        Self
    }

    fn default_user_id(&self) -> String {
        Self::COMPAT_USER_ID.to_string()
    }

    fn default_thread_id(&self, user_id: &str) -> String {
        let _ = user_id;
        Self::COMPAT_THREAD_ID.to_string()
    }

    fn default_workspace_id(&self) -> String {
        Self::COMPAT_WORKSPACE_ID.to_string()
    }
}

impl MemoryIdentityResolver for LocalIdentityResolver {
    fn resolve_actor(&self, input: &MemoryActorInput) -> ResolvedMemoryActor {
        let user_id = input
            .user_id
            .clone()
            .unwrap_or_else(|| self.default_user_id());
        let thread_id = input
            .thread_id
            .clone()
            .unwrap_or_else(|| self.default_thread_id(&user_id));
        let mut workspace_ids: BTreeSet<String> = input.workspace_ids.iter().cloned().collect();
        if workspace_ids.is_empty() {
            workspace_ids.insert(self.default_workspace_id());
        }
        ResolvedMemoryActor {
            user_id,
            thread_id,
            workspace_ids,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LocalIdentityResolver, MemoryActorInput, MemoryIdentityResolver};

    #[test]
    fn local_identity_resolver_uses_explicit_ids_when_present() {
        let resolver = LocalIdentityResolver::new("/tmp/project");
        let actor = resolver.resolve_actor(&MemoryActorInput {
            user_id: Some("user_1".to_string()),
            thread_id: Some("thread_1".to_string()),
            workspace_ids: vec!["ws_a".to_string(), "ws_b".to_string()],
        });
        assert_eq!(actor.user_id, "user_1");
        assert_eq!(actor.thread_id, "thread_1");
        assert!(actor.workspace_ids.contains("ws_a"));
        assert!(actor.workspace_ids.contains("ws_b"));
    }

    #[test]
    fn local_identity_resolver_synthesizes_compatibility_ids() {
        let resolver = LocalIdentityResolver::new("/tmp/project");
        let actor = resolver.resolve_actor(&MemoryActorInput::default());
        assert_eq!(actor.user_id, "__user__");
        assert_eq!(actor.thread_id, "__thread__");
        assert_eq!(actor.workspace_ids.len(), 1);
        assert_eq!(
            actor.workspace_ids.iter().next().unwrap(),
            "__compat_workspace__"
        );
    }
}
