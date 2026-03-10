use std::sync::Arc;

use anyhow::Result;

use crate::runtime::RuntimeMiddleware;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuntimeMiddlewareSlot {
    TodoList,
    Memory,
    Skills,
    FilesystemRuntime,
    Subagents,
    Summarization,
    PromptCaching,
    PatchToolCalls,
    User,
    Hitl,
}

#[derive(Clone)]
pub struct RuntimeMiddlewareDescriptor {
    pub slot: RuntimeMiddlewareSlot,
    pub label: &'static str,
    pub middleware: Arc<dyn RuntimeMiddleware>,
}

#[derive(Default)]
pub struct RuntimeMiddlewareAssembler {
    items: Vec<RuntimeMiddlewareDescriptor>,
}

impl RuntimeMiddlewareAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(
        &mut self,
        slot: RuntimeMiddlewareSlot,
        label: &'static str,
        middleware: Arc<dyn RuntimeMiddleware>,
    ) {
        self.items.push(RuntimeMiddlewareDescriptor {
            slot,
            label,
            middleware,
        });
    }

    pub fn push_user(&mut self, label: &'static str, middleware: Arc<dyn RuntimeMiddleware>) {
        self.push(RuntimeMiddlewareSlot::User, label, middleware);
    }

    pub fn build(self) -> Result<Vec<Arc<dyn RuntimeMiddleware>>> {
        sort_runtime_middlewares(self.items)
    }
}

pub fn sort_runtime_middlewares(
    mut items: Vec<RuntimeMiddlewareDescriptor>,
) -> Result<Vec<Arc<dyn RuntimeMiddleware>>> {
    items.sort_by(|a, b| a.slot.cmp(&b.slot).then_with(|| a.label.cmp(b.label)));
    let mut last: Option<RuntimeMiddlewareSlot> = None;
    for d in items.iter() {
        if Some(d.slot) == last && d.slot != RuntimeMiddlewareSlot::User {
            anyhow::bail!("duplicate runtime middleware slot: {:?}", d.slot);
        }
        last = Some(d.slot);
    }
    Ok(items.into_iter().map(|d| d.middleware).collect())
}
