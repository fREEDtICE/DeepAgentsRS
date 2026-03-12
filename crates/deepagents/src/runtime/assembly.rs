use std::sync::Arc;

use anyhow::Result;

use crate::runtime::RuntimeMiddleware;

/// Runtime 中间件装配与排序逻辑。
///
/// 作用：
/// - 将各类 `RuntimeMiddleware` 按“槽位（slot）+ 标签（label）”汇总
/// - 在 `build()` 时统一排序，得到稳定、可预测的中间件执行顺序
/// - 防止同一非 User 槽位被重复注册，避免中间件链条语义冲突
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuntimeMiddlewareSlot {
    /// 负责 todo list 相关的运行时处理。
    TodoList,
    /// 负责 memory 相关的运行时处理。
    Memory,
    /// 负责 skills 相关的运行时处理。
    Skills,
    /// 负责文件系统/运行时文件工具相关的处理。
    FilesystemRuntime,
    /// 负责 subagents 相关的运行时处理。
    Subagents,
    /// 负责对话/状态摘要相关的运行时处理。
    Summarization,
    /// 负责 prompt 缓存相关的运行时处理。
    PromptCaching,
    /// 负责 tool call 修补与规范化相关的运行时处理。
    PatchToolCalls,
    /// 用户自定义中间件槽位（允许多个）。
    User,
    /// HITL（Human-in-the-loop）相关的运行时处理。
    Hitl,
}

/// 一个中间件的装配描述。
///
/// - `slot`：用于确定排序与去重规则
/// - `label`：用于同槽位内进一步排序（并作为调试/诊断标识）
/// - `middleware`：具体中间件实现
#[derive(Clone)]
pub struct RuntimeMiddlewareDescriptor {
    pub slot: RuntimeMiddlewareSlot,
    pub label: &'static str,
    pub middleware: Arc<dyn RuntimeMiddleware>,
}

/// 中间件装配器：收集描述并在 build 时输出排序后的中间件链。
#[derive(Default)]
pub struct RuntimeMiddlewareAssembler {
    items: Vec<RuntimeMiddlewareDescriptor>,
}

impl RuntimeMiddlewareAssembler {
    /// 创建装配器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个中间件到指定槽位。
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

    /// 注册用户自定义中间件（固定落在 User 槽位，且允许多个）。
    pub fn push_user(&mut self, label: &'static str, middleware: Arc<dyn RuntimeMiddleware>) {
        self.push(RuntimeMiddlewareSlot::User, label, middleware);
    }

    /// 输出排序后的中间件链（`Vec<Arc<dyn RuntimeMiddleware>>`）。
    pub fn build(self) -> Result<Vec<Arc<dyn RuntimeMiddleware>>> {
        sort_runtime_middlewares(self.items)
    }
}

/// 对中间件进行排序并校验槽位唯一性。
///
/// 排序规则：
/// - 先按 slot（枚举序）排序，再按 label 字典序排序
///
/// 校验规则：
/// - 除 `User` 槽位外，不允许出现重复 slot（避免同一阶段多重拦截导致语义不确定）
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
