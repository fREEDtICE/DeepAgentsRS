# Phase 7 详细迭代计划（MemoryMiddleware：记忆抽象与最小实现）

适用范围：本计划面向 [ITERATION_PLAN.md](ITERATION_PLAN.md#L227-L244) 的 Phase 7。目标是在 Rust 版 deepagents 已具备的 runtime/tool/state/subagents 基线之上，引入 **可插拔的记忆存储抽象（MemoryStore）** 与 **最小可用的本地实现（文件型记忆）**，并把 **生命周期、容量策略、隐私边界、失败语义** 固化为可回归契约，为后续 Phase 8（Summarization）与“长期会话/恢复历史”提供稳定支点。

对齐锚点（必须显式对齐，避免只对齐文案而偏离行为）：

- 总计划的 Phase 7 定义：[ITERATION_PLAN.md](ITERATION_PLAN.md#L227-L244)
- Python 参考实现（行为参考，不依赖实现细节）：[memory.py](../../../deepagents/libs/deepagents/deepagents/middleware/memory.py)
- Python 调研结论（Memory 的可观察语义与私有 state）：[RESEARCH_DEEPAGENTS_PYTHON.md](../RESEARCH_DEEPAGENTS_PYTHON.md#L216-L227)
- Rust state 承载方式（强类型 + extra 扩展区）：[state.rs](../../crates/deepagents/src/state.rs)
- Rust 子代理隔离（排除 `memory_contents`）：[protocol.rs](../../crates/deepagents/src/subagents/protocol.rs#L15-L85)
- Rust runtime middleware hook（`before_run/patch_provider_step/handle_tool_call`）：[protocol.rs](../../crates/deepagents/src/runtime/protocol.rs#L99-L115)

本计划默认采用 **Trait-first + privacy-first + 可观测输出优先** 的策略：

- Trait-first：先冻结 MemoryStore/MemoryMiddleware 的对外契约与错误码，再落实现与测试，避免 Phase 8/9 返工。
- Privacy-first：明确“哪些内容只给模型看、哪些可以对外输出/持久化、哪些绝不跨 subagent 传播”。
- 可观测输出优先：所有关键分支（加载/跳过/截断/失败）都要可通过结构化输出或测试断言观测到。

---

## 0. 完成定义（Definition of Done）

Phase 7 完成必须同时满足：

- MemoryStore 抽象已固化（可插拔）：
  - 提供最小能力集合：加载（load）、查询（get/query）、写入（put/upsert）、淘汰（evict/compact）语义明确且可测试。
  - 序列化格式稳定：持久化文件格式、版本字段、向后兼容策略固定。
  - 生命周期与容量策略固定：何时加载、何时写回、缓存策略、预算与淘汰策略均可回归。
- 最小本地实现可用（文件型记忆）：
  - 默认读取/写入一个“记忆文件”（推荐对齐 AGENTS.md 生态，或 `.deepagents/AGENTS.md`），并可配置多源（sources）与合并策略。
  - 与 Rust sandbox 安全口径一致：默认不允许越过 root（或必须显式 opt-in），并具备 symlink/路径穿越防护。
- MemoryMiddleware 可用（Rust）：
  - 在运行开始前加载记忆（一次性），注入到模型可见 system prompt（固定 marker，幂等）。
  - 记忆内容属于私有 state：不出现在对外 state 输出，不写入 state-file，不随 subagent 继承（对齐 Python 的 `PrivateStateAttr` + excluded keys 意图）。
  - 加载失败语义固定：`file_not_found` 可忽略，其它错误默认视为硬失败并终止运行（对齐 Python 可观察行为）。
- 测试门禁：
  - 单测：写入/查询/淘汰策略正确（对齐总计划验收）。
  - 集成/E2E：能断言 system prompt 注入与隐私边界（输出中不泄露 `memory_contents`）。
  - `cargo test` 全通过。

---

## 1. 范围与非目标（Scope / Non-goals）

### 1.1 范围（Phase 7 必做）

- MemoryStore trait（可插拔记忆存储接口）与最小本地实现（文件型）。
- 记忆序列化格式（版本化、可扩展、可回归）。
- 生命周期策略（加载时机、缓存时机、写回时机、与 state/会话的关系）。
- 容量预算与淘汰策略（例如按字节/条目数/时间窗口的驱逐）。
- MemoryMiddleware（将记忆注入模型可见上下文 + 私有 state 处理 + 失败语义固化）。
- CLI/ACP 的最小接入点（至少 CLI `run` 能启用 memory；ACP 至少保证 state 输出不泄露私有记忆）。

### 1.2 非目标（Phase 7 不做，但需留好接口）

- 向量检索/embedding/语义搜索型长期记忆（可在后续阶段单独演进）。
- 大工具结果 offload（`/large_tool_results/...`）的完整实现（Phase 7 不以此为阻塞）。
- SummarizationEvent（Phase 8）与历史裁剪/落盘（Phase 7 只需保证 memory 不破坏后续可实现性）。
- 多后端路由与虚拟路径模型（CompositeBackend 对齐）在 Phase 7 不强行落地，但需在接口上避免“未来无法接入”。

---

## 2. 当前系统基线与 Phase 7 缺口

### 2.1 已有能力（可复用）

- Rust runtime middleware 具备 `before_run`（一次性加载/注入最合适的入口），见 [protocol.rs](../../crates/deepagents/src/runtime/protocol.rs#L99-L115)。
- Rust 子代理隔离已把 `memory_contents` 纳入排除键集合（与 Python `_EXCLUDED_STATE_KEYS` 对齐），见 [protocol.rs](../../crates/deepagents/src/subagents/protocol.rs#L15-L85)。
- Rust 后端（LocalSandbox）对 `file_not_found` 等错误已使用稳定字符串码（`BackendError::Other("file_not_found")`），可直接对齐 Python 的忽略规则，见 [local.rs](../../crates/deepagents/src/backends/local.rs#L98-L123)。
- Rust state 采用 `filesystem + extra` 的结构，便于先把 Memory 的 public/可过滤信息落在 `extra`，再逐步收敛为更强类型，见 [state.rs](../../crates/deepagents/src/state.rs#L6-L12)。

### 2.2 Phase 7 必补缺口

- 缺 MemoryStore 抽象与本地持久化实现（总计划验收要求“写入/查询/淘汰”）。
- 缺“私有 state”的稳定机制：
  - Python 通过 `PrivateStateAttr` 确保 `memory_contents` 不出现在最终 state；Rust 目前没有对应的结构化能力，只能靠约定或额外过滤。
- 缺 MemoryMiddleware：
  - Python 在 `before_agent` 阶段加载并在每次 model call 前注入 system prompt；Rust 需要在 `before_run` 阶段实现等价注入/幂等策略。
- 缺容量与失败语义固化：
  - 需要明确记忆文件过大/损坏/权限不足等情形的行为，且这些行为必须能回归。

---

## 3. 对外契约（必须冻结）

Phase 7 的关键在于冻结“记忆是什么、何时生效、如何存取、如何失败、如何不泄露”。

### 3.1 记忆的产品语义（Memory vs Skills）

冻结定义（对齐 Python 模型）：

- memory：**始终加载**（always-on），为模型提供稳定的长期背景与偏好约束；属于“上下文注入 + 可编辑的持久文本”。
- skills：按需调用（on-demand workflow），属于“宏工具/工具组合”，其输出是工具调用轨迹与结果。

Phase 7 只解决 memory，不扩展 skills 语义。

### 3.2 记忆来源与合并规则（sources → combined memory）

对齐 Python 行为并冻结：

- `sources: Vec<String>` 为有序列表，按顺序加载。
- 合并策略：按顺序串接所有成功加载的内容；后加载的 source 追加在后（不覆盖、不去重）。
- `file_not_found`：忽略（允许用户给出“可选 source”，例如用户目录或项目目录不存在时不阻塞）。
- 其它错误：默认硬失败（终止本次运行并返回可诊断错误），对齐 Python `ValueError("Failed to download ...")` 的“硬失败”性质。

补充（Rust 需要明确的安全口径）：

- sources 的路径解析/越界策略必须二选一并固化（建议默认更安全）：
  - 默认（推荐）：sources 必须在 sandbox `root` 之内，越界即拒绝并失败。
  - 可选（显式 opt-in）：允许读取 host 路径（例如 `~/.deepagents/AGENTS.md`），但必须通过显式开关启用，并且只允许读取 `AGENTS.md`（或强约束到白名单文件名集合），降低误读敏感文件的风险。

### 3.3 system prompt 注入契约（可观察、幂等、可诊断）

对齐 Python `MEMORY_SYSTEM_PROMPT` 的意图并冻结如下可观察行为：

- 记忆注入必须出现在模型可见 system prompt 中，且具备固定边界标签，便于测试与排障：
  - `<agent_memory>...</agent_memory>`：包含合并后的记忆主体（建议包含 source 路径 header）。
  - `<memory_guidelines>...</memory_guidelines>`：包含“如何更新记忆、何时不更新、禁止存储秘密”的规则。
- 注入必须幂等：同一次 `run` 或“恢复继续跑”不应重复追加相同记忆块。
  - 推荐使用固定 marker（例如 `DEEPAGENTS_MEMORY_INJECTED_V1`）来识别是否已注入。
- 注入位置固定：建议作为 system message 的追加片段（append），不应改写用户 messages；并且不依赖 provider 的实现细节。

### 3.4 私有 state 契约（必须对齐 Python 的 PrivateStateAttr 意图）

冻结以下约束（这是 Phase 7 的关键差异点）：

- `memory_contents` 属于私有数据：
  - 不出现在 `RunOutput.state`（对外输出）。
  - 不写入 CLI 的 `--state-file`（持久化 checkpoint）。
  - 不通过 ACP `session_state` 等 API 返回给客户端。
  - 不随 subagent 继承（Rust 已排除该键，但 Phase 7 仍需确保“不会误落入 public state”）。
- 但运行期必须可用：MemoryMiddleware 需要在后续 provider 调用中读取它，以便（可选）实现“每轮注入”或诊断输出。

推荐实现策略（冻结为可回归契约的一部分）：

- 将 AgentState 拆分为：
  - public：可序列化/可持久化/可回传（现有 `filesystem + extra`）。
  - private：仅运行期可见，序列化时跳过（新增 `private` 容器）。

### 3.5 MemoryStore 的最小能力集合与错误语义

为了满足总计划验收“写入/查询/淘汰策略正确”，MemoryStore 必须至少支持：

- `put`: 写入一条记忆条目（key/value 或 structured entry）。
- `get/query`: 读取（按 key 或按 prefix/tag 的简单查询即可，Phase 7 不要求语义检索）。
- `evict/compact`: 在容量超限时可驱逐（按 LRU/TTL/条目数/字节预算之一）。

错误语义冻结：

- I/O 或格式错误必须区分为可诊断 code（而不是只有字符串），至少区分：
  - `memory_not_found`（可选：用于初始化空 store）
  - `memory_permission_denied`
  - `memory_corrupt`
  - `memory_io_error`
  - `memory_quota_exceeded`

---

## 4. 数据流（Data Flow）

### 4.1 运行期数据流（MemoryMiddleware）

1) CLI/ACP 构建 runtime，并装配 MemoryMiddleware（含 sources 与 store 配置）。
2) `before_run`：
   - 若 private state 标记已加载（例如 `private["memory_loaded"]=true` 或 `private["memory_contents"]` 存在），则跳过加载（幂等）。
   - 否则调用 MemoryStore 加载 sources，并将内容写入 private state（`memory_contents`）。
   - 根据内容生成注入块，并追加到 system message（或插入一个 system message）。
3) runtime 进入 provider loop：模型在每轮可看到注入后的 system prompt。
4) 模型更新记忆：
   - 对齐 Python：通过 `edit_file/write_file` 修改记忆文件（AGENTS.md）。
   - Phase 7 默认不要求“同一次 run 内重新加载并反映到 system prompt”，即记忆更新在下一次 run/下一次会话重建时生效（与 Python 当前实现一致）。

### 4.2 持久化数据流（MemoryStore）

- 文件型 store（最小实现）建议使用一个版本化 JSON 文件保存“结构化条目”，并可选将其渲染/同步到 AGENTS.md（两种模式二选一，见 5.2）。
- 读路径：启动时从文件 load → 内存索引 → query/put。
- 写路径：put 后按策略写回（同步写/批量写），写回失败要可诊断且不引入 silent corruption。

---

## 5. 核心架构与数据模型（Trait-first）

### 5.1 模块拆分（建议）

- `memory::protocol`：
  - `MemoryStore` trait
  - `MemoryError/MemoryErrorCode`
  - `MemoryEntry/MemoryQuery/MemoryPolicy`
- `memory::store_file`：
  - `FileMemoryStore`（最小落地）
  - 序列化格式版本化（v1）
- `runtime::memory_middleware`：
  - `MemoryMiddleware`（RuntimeMiddleware 实现）
  - 注入模板/marker/诊断输出

### 5.2 “文件型记忆”两种落地形态（二选一并固化）

Phase 7 需要同时满足：
1) 对齐 Python 的“AGENTS.md 作为 always-on 记忆上下文”的体验；
2) 满足总计划验收的“写入/查询/淘汰策略正确”（这更像结构化 store）。

因此建议在 Phase 7 明确选择其一作为默认（另一种作为兼容/扩展）：

**方案 A（推荐默认）：结构化 store + 渲染到 AGENTS.md**

- Store 真正持久化的是 `memory_store.json`（结构化条目，易测试、易淘汰、易查询）。
- 每次启动（或每次写入）将结构化条目渲染成 `AGENTS.md`（模型可读的 Markdown），用于 MemoryMiddleware 注入。
- 优点：验收“写入/查询/淘汰”容易做对；同时对齐 AGENTS.md 生态（最终注入的是 AGENTS.md 或等价渲染）。
- 风险：需要定义渲染格式与同步策略（何时写回、冲突如何处理）。

**方案 B：直接把 AGENTS.md 当作 store（append/edit）**

- Store 的写入/淘汰需要对 Markdown 做结构化约束（例如前缀/分隔符/自动生成区块），否则很难可靠淘汰与查询。
- 优点：最贴近 Python 的“编辑文件=更新记忆”路径。
- 风险：淘汰/查询很难做到稳定回归，容易在复杂编辑下失效。

本计划默认采用 **方案 A**，并提供与 Python 的兼容说明：模型仍可通过 `edit_file` 修改 AGENTS.md，但那属于“人工维护区”，不保证会被结构化淘汰算法重写；结构化记忆写入应提供一个明确入口（可先仅供系统内部/CLI 子命令使用，见 6.4）。

### 5.3 MemoryStore trait（建议形状）

以下为建议契约（以最终实现为准，但必须覆盖验收点）：

```rust
pub trait MemoryStore: Send + Sync {
    fn name(&self) -> &str;

    fn policy(&self) -> MemoryPolicy;

    async fn load(&self) -> Result<(), MemoryError>;
    async fn flush(&self) -> Result<(), MemoryError>;

    async fn put(&self, entry: MemoryEntry) -> Result<(), MemoryError>;
    async fn get(&self, key: &str) -> Result<Option<MemoryEntry>, MemoryError>;
    async fn query(&self, q: MemoryQuery) -> Result<Vec<MemoryEntry>, MemoryError>;

    async fn evict_if_needed(&self) -> Result<MemoryEvictionReport, MemoryError>;
}
```

必要字段建议：

- `MemoryEntry`：
  - `key: String`（稳定标识，例如 `user_preference/js_examples`）
  - `value: String`（简短、无秘密）
  - `tags: Vec<String>`（可选，用于简单 query）
  - `created_at/updated_at: String`（ISO8601）
  - `access_count/last_accessed_at`（用于 LRU/频控，最小可选）
- `MemoryPolicy`（容量策略）：
  - `max_entries`
  - `max_bytes_total`
  - `eviction: { Lru | Fifo | Ttl }`（至少一种）

### 5.4 序列化格式（FileMemoryStore v1）

推荐 v1 文件：`<root>/.deepagents/memory_store.json`（或可配置）。

格式（建议，需冻结字段与版本策略）：

```json
{
  "version": 1,
  "policy": {
    "max_entries": 200,
    "max_bytes_total": 200000
  },
  "entries": [
    {
      "key": "user_preference/code_language",
      "value": "User prefers JavaScript examples when available.",
      "tags": ["preference", "language"],
      "created_at": "2026-03-09T12:00:00Z",
      "updated_at": "2026-03-09T12:00:00Z",
      "last_accessed_at": "2026-03-09T12:00:00Z",
      "access_count": 3
    }
  ]
}
```

版本化策略冻结：

- `version` 必须存在。
- 读取到更高版本：拒绝并返回 `memory_corrupt: unsupported_version`（可诊断）。
- 缺字段：默认拒绝（deny unknown fields + 必填字段检查），避免 silent corruption。
- 向后兼容：仅允许“新增可选字段”，并在 v2 时明确迁移路径。

### 5.5 MemoryMiddleware（RuntimeMiddleware）建议形状

MemoryMiddleware 的职责被刻意限制为“加载/注入/诊断”，不承担复杂的写入/淘汰：

- 输入：
  - sources（AGENTS.md 列表，按 3.2 规则合并）
  - MemoryStore（用于结构化 store 的 load/query，可选渲染 AGENTS.md）
  - 注入模板/marker/预算（最大注入字节数）
  - 安全策略（是否允许 sources 越界）
- 输出：
  - 修改后的 messages（system prompt 注入）
  - private state：`memory_contents`（source → content）与 `memory_diagnostics`（不含内容，只含统计）

---

## 6. 核心细节（必须提前冻结）

### 6.1 路径解析与安全策略

必须冻结以下规则，否则会影响用户可预期性与安全口径：

- `sources` 允许的语法：
  - 相对路径：相对于 `root`（推荐）或当前工作目录（二选一并文档化；建议 root）。
  - 绝对路径：默认禁止越过 root；若启用 `allow_host_paths` 则允许（仍需 canonicalize + symlink 防逃逸）。
  - `~/...`：仅在启用 `allow_host_paths` 时展开（使用 `HOME` 环境变量），展开后按绝对路径规则处理。
- 安全检查：
  - canonicalize（或等价 normalize）后必须满足 root 前缀（除非 allow_host_paths）。
  - 拒绝 symlink 作为 source 文件或其父目录链中的任一节点（防止“表面在 root 内，实际跳出 root”）。
  - 默认只允许 basename 为 `AGENTS.md`（或白名单集合），进一步降低误读风险（该规则必须写入文档与测试）。

### 6.2 失败语义（与 Python 对齐 + Rust 的可诊断性）

冻结：

- `file_not_found`：忽略（并记录到 diagnostics：skipped_not_found）。
- 其它错误：
  - 默认硬失败：`before_run` 返回 error，runtime 终止，`RunOutput.error.code="middleware_error"`（当前 runtime 行为），message 中必须携带可诊断的 memory error code（例如 `memory_load_failed: ...`）。
  - 可选配置（非默认）：`skip_invalid_sources=true`，将错误降级为告警并继续（为“可选用户目录”场景保留出路），但必须强制输出 diagnostics 以避免 silent ignore。

### 6.3 容量预算（注入预算 vs store 预算）

需要区分两类预算并冻结策略：

- **注入预算**（模型上下文预算）：
  - `max_injected_bytes_total`：限制最终注入 system prompt 的字节数（或按 token 估算）。
  - 超限策略：截断（保留头部 + 尾部预览 + 明确提示“已截断”），并记录到 diagnostics（truncated=true）。
  - Phase 7 不做 offload（注入内容不能通过“文件引用”替代，除非后续引入新的 tool 引导读取）。
- **store 预算**（持久化容量预算）：
  - `max_entries/max_bytes_total`：由 MemoryPolicy 管控。
  - 超限策略：执行 eviction（LRU/FIFO/TTL），生成可回归的 eviction report。

### 6.4 “写入记忆”的入口（如何满足验收但不破坏现有工具契约）

为了满足“写入/查询/淘汰”验收，同时不引入新的不安全工具，推荐提供以下最小入口：

- CLI 子命令（建议）：
  - `deepagents memory put --key ... --value ... [--tag ...]`
  - `deepagents memory query --tag ...` 或 `--prefix ...`
  - `deepagents memory compact`（触发 eviction/flush）
- 运行期（模型侧）：
  - Phase 7 默认不新增“remember”工具，避免模型直接写结构化 store（可能泄露秘密/破坏格式）。
  - 仍对齐 Python：模型更新记忆的主路径是 `edit_file` 修改 AGENTS.md（这是人类可审计的文本），结构化 store 的写入由人类或产品层完成。

如果必须让模型可写结构化记忆，必须新增独立工具并强制：
1) deny-by-default；
2) 对 value 做 secrets 检测与长度限制；
3) 写入路径固定且不可越界；
这不属于 Phase 7 默认范围。

---

## 7. 详细迭代拆解（里程碑）

### M0：冻结契约与验收编号（文档优先）

- 输出
  - 冻结 sources 合并/失败语义（3.2、6.2）
  - 冻结私有 state 口径与实现策略（3.4）
  - 冻结 FileMemoryStore v1 序列化格式与版本策略（5.4）
  - 冻结容量策略：注入预算与 store 预算（6.3）
- 验收
  - 本文契约可直接映射到测试用例，无歧义

### M1：实现 MemoryStore trait + FileMemoryStore（单测）

- 任务
  - 定义 MemoryStore/MemoryEntry/MemoryPolicy/MemoryError
  - 实现 FileMemoryStore：load/flush/put/get/query/evict
  - 选择并固化 eviction 策略（推荐 LRU 或 FIFO，至少一种）
  - 严格反序列化策略（deny unknown fields + version 检查）
- 验收（单测）
  - 写入/查询正确
  - 超限时淘汰正确且可断言（eviction report）
  - 损坏文件/不支持版本返回可诊断错误

### M2：实现“渲染到 AGENTS.md”（对齐 Python 体验）

- 任务
  - 将结构化条目渲染为稳定 Markdown（固定标题、固定分隔符）
  - 渲染输出路径固定（默认 `<root>/.deepagents/AGENTS.md`，可配置）
  - 渲染策略固定：仅覆盖“自动生成区”，不覆盖用户手写区（避免破坏手工维护的说明）
- 验收（单测 + fixture）
  - 给定 entries，渲染输出稳定（snapshot 测试）
  - compact/evict 后渲染结果随之更新

### M3：实现 MemoryMiddleware（加载一次 + system 注入 + 私有 state）

- 任务
  - `before_run`：
    - load sources（AGENTS.md）并合并
    - 应用注入预算与截断策略
    - 注入 system prompt（marker 幂等）
    - 写入 private state（memory_contents + diagnostics）
  - 私有 state 机制落地：
    - 新增 `AgentState.private`（或等价机制），并确保其不参与序列化/回传/持久化
- 验收（集成测）
  - `RunOutput.state` 中不含 `memory_contents`
  - system prompt 中包含 `<agent_memory>` 与 marker

### M4：CLI/ACP 接入与可观测诊断

- 任务
  - CLI `run` 增加 memory 配置入口（sources、allow_host_paths、预算），并确保默认安全
  - CLI/ACP 输出增加 diagnostics（不含内容）：
    - 已加载 sources、跳过 not_found 数量、截断与字节数统计
  - state-file 写入时明确过滤 private state（或 private 不参与序列化）
- 验收（黑盒/E2E）
  - 运行时可断言 memory 注入发生且不泄露内容

### M5：安全矩阵与负测补齐

- 任务
  - symlink 逃逸负测（source 指向 root 外）
  - allow_host_paths=off 时拒绝 `~/...`、绝对越界路径
  - 记忆内容 secrets 红线：至少保证注入模板明确禁止存储密钥，并在 CLI `memory put` 做简单阻断（可选但推荐）
- 验收
  - 安全负测通过，错误码可诊断

---

## 8. 测试计划（以验收为主线）

### 8.1 单元测试（必须）

- FileMemoryStore
  - put/get/query 行为
  - eviction（容量超限）
  - 序列化版本与 deny unknown fields
  - corrupt 文件处理（返回 `memory_corrupt`）
- MemoryMiddleware
  - sources 合并顺序
  - `file_not_found` 忽略
  - 其它错误硬失败（默认）
  - 注入幂等（marker）
  - 注入预算截断策略（头尾保留 + 明确提示）
  - 私有 state 不出现在 public state

### 8.2 集成/E2E（推荐）

- CLI `run`：
  - 指定 sources 后，stdout 的 run 输出（JSON）可断言 system prompt 已注入（通过 trace 或快照）
  - state-file 中不出现 `memory_contents`
- ACP（若 Phase 7 覆盖 ACP）：
  - `GET /session_state` 不包含 `memory_contents`

---

## 9. 风险与取舍（提前声明）

- 私有 state 机制是“跨阶段基础设施”：Phase 7 引入后，Phase 6/8 可能也会迁移部分敏感字段到 private；需要明确迁移策略，避免 breaking change。
- sources 越界（`~/.deepagents/AGENTS.md`）与 sandbox root 的冲突：若完全禁止，将无法对齐 Python 示例；若允许则存在误读敏感文件风险。本计划采用“默认禁止 + 显式 opt-in + 白名单文件名 + symlink 防护”的折中。
- “模型通过 edit_file 更新记忆”与“结构化 store 作为验收对象”之间存在语义裂缝：本计划默认将结构化写入入口放在 CLI（人类/产品层），并把 AGENTS.md 作为渲染产物与模型可见载体，从而兼顾可回归与可审计。

---

## 10. 交付物清单（Deliverables）

- 文档
  - Phase 7 详细迭代计划（本文）
  - MemoryStore v1 序列化格式说明（可附在本文或独立文档）
- 代码（实现阶段产出，应与本文一致）
  - `memory::protocol`：MemoryStore trait + error/policy/types
  - `memory::store_file`：FileMemoryStore（v1）
  - `runtime::memory_middleware`：MemoryMiddleware（before_run 注入 + private state）
  - CLI：`deepagents memory *` 子命令 + `run` memory 配置入口
  - 测试：store 单测 + middleware 单测 + 必要的集成/E2E
