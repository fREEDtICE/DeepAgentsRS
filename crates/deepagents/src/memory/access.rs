use crate::memory::identity::ResolvedMemoryActor;
use crate::memory::protocol::{MemoryEntry, MemoryScopeType};

pub fn can_access_scope(
    scope_type: MemoryScopeType,
    scope_id: &str,
    actor: &ResolvedMemoryActor,
) -> bool {
    match scope_type {
        MemoryScopeType::Thread => actor.thread_id == scope_id,
        MemoryScopeType::User => actor.user_id == scope_id,
        MemoryScopeType::Workspace => actor.workspace_ids.contains(scope_id),
        MemoryScopeType::System => false,
    }
}

pub fn can_read_entry(entry: &MemoryEntry, actor: &ResolvedMemoryActor) -> bool {
    can_access_scope(entry.scope_type, &entry.scope_id, actor)
}

pub fn can_write_scope(
    scope_type: MemoryScopeType,
    scope_id: &str,
    actor: &ResolvedMemoryActor,
) -> bool {
    can_access_scope(scope_type, scope_id, actor)
}

pub fn filter_readable_entries(
    entries: Vec<MemoryEntry>,
    actor: &ResolvedMemoryActor,
) -> Vec<MemoryEntry> {
    entries
        .into_iter()
        .filter(|entry| can_read_entry(entry, actor))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{can_read_entry, can_write_scope};
    use crate::memory::identity::ResolvedMemoryActor;
    use crate::memory::{MemoryEntry, MemoryScopeType};

    fn actor() -> ResolvedMemoryActor {
        let mut workspace_ids = BTreeSet::new();
        workspace_ids.insert("ws_a".to_string());
        ResolvedMemoryActor {
            user_id: "user_1".to_string(),
            thread_id: "thread_1".to_string(),
            workspace_ids,
        }
    }

    #[test]
    fn read_access_is_scope_aware() {
        let actor = actor();
        let mut thread_entry = MemoryEntry::new("thread", "value");
        thread_entry.scope_type = MemoryScopeType::Thread;
        thread_entry.scope_id = "thread_1".to_string();
        assert!(can_read_entry(&thread_entry, &actor));

        let mut user_entry = MemoryEntry::new("user", "value");
        user_entry.scope_type = MemoryScopeType::User;
        user_entry.scope_id = "user_1".to_string();
        assert!(can_read_entry(&user_entry, &actor));

        let mut workspace_entry = MemoryEntry::new("workspace", "value");
        workspace_entry.scope_type = MemoryScopeType::Workspace;
        workspace_entry.scope_id = "ws_a".to_string();
        assert!(can_read_entry(&workspace_entry, &actor));

        let mut blocked = MemoryEntry::new("blocked", "value");
        blocked.scope_type = MemoryScopeType::User;
        blocked.scope_id = "user_2".to_string();
        assert!(!can_read_entry(&blocked, &actor));
    }

    #[test]
    fn write_access_is_scope_aware() {
        let actor = actor();
        assert!(can_write_scope(MemoryScopeType::Thread, "thread_1", &actor));
        assert!(can_write_scope(MemoryScopeType::User, "user_1", &actor));
        assert!(can_write_scope(MemoryScopeType::Workspace, "ws_a", &actor));
        assert!(!can_write_scope(MemoryScopeType::User, "user_2", &actor));
        assert!(!can_write_scope(
            MemoryScopeType::System,
            "__system__",
            &actor
        ));
    }
}
