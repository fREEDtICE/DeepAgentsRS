# Phase 1 详细迭代计划（契约固化 + FilesystemMiddleware：状态与 schema）

适用范围：本计划面向 [ITERATION_PLAN.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/ITERATION_PLAN.md#L34-L55) 的 Phase 1。目标是在 Phase 0 的“工具可用闭环”基础上，把 **契约（schema/错误码/截断）** 与 **FilesystemMiddleware（state + reducer）** 固化为 Rust 侧可扩展（Trait-first）、可测试、可持续演进的公共能力层，为 Phase 1.5+ 的 runtime/provider/memory 等打基础。

## 0. 当前系统基线（Phase 1 启动时的真实状态）

- 已具备：本地 `LocalSandbox` + 默认工具集 + CLI `tool` 子命令 + 单元测试 + CLI smoke E2E
- 尚缺失（Phase 1 需要补齐）：
  - `FilesystemState` 数据模型与 reducer
  - 中间件执行契约（Rust 端 `Middleware` 目前只是空 trait）
  - 工具 schema 的稳定定义与回归机制（字段/默认值/错误码/输出模式）
  - “state 可观测接口”（给 E2E/用户/后续 runtime 用）：session/`--state-file` 等

## 1. Phase 1 完成定义（Definition of Done）

Phase 1 完成必须同时满足：

- **Trait-first**：FilesystemMiddleware、StateStore/Checkpoint、Reducer、ToolSchema 等均以 trait 暴露公共边界；默认实现只是参考实现
- **FilesystemState**：具备最小可用的 files 快照模型，支持“写入/编辑/删除标记/合并”语义，并有单测覆盖
- **Reducer**：定义并实现 state 合并规则（覆盖覆盖、删除、并发更新的确定性行为）
- **Schema 固化**：
  - 工具输入/输出使用稳定的 Rust struct（serde）作为事实来源
  - 默认值、错误码、输出模式（grep）与截断信号在端到端路径中一致
  - 提供 schema 回归基线（snapshot 或测试用例）
- **端到端可观测**：提供一种稳定方式让调用方读取/写回 state（建议 `--state-file`；或 session 子命令）
- **测试**：
  - 单测：reducer 覆盖覆盖/删除/并发更新
  - 集成测试：tool 调用 → state 更新 → 后续 tool 使用该 state（最小链路）
  - Phase 1 E2E 计划文档已存在：[E2E_PHASE1.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/E2E_PHASE1.md)

## 2. 核心设计决策（必须先定，避免返工）

### 2.1 Trait-first：公共扩展点与默认实现的关系

Phase 1 需要新增/固化的扩展点（建议以 trait 暴露在 `deepagents` core crate）：

- `FilesystemState`：数据结构（serde 可序列化）
- `StateReducer<S>`：合并策略（纯函数：`fn reduce(old, delta) -> new`）
- `FilesystemMiddleware`：负责把工具调用结果转换为 `FilesystemStateDelta` 并交给 reducer
- `StateStore`（或 `CheckpointStore`）：可选；用于持久化 state（Phase 1 以文件实现为默认）
- `ToolSchema`/`ToolSpec`：工具的输入输出类型与元信息（名称/描述/输入 struct/输出 struct）

默认实现：

- `LocalFilesystemMiddleware`：默认文件系统状态中间件
- `JsonFileStateStore`：默认 state-file 存储（供 CLI/E2E）

### 2.2 FilesystemState：最小字段集合与语义

建议 Phase 1 先把 files 快照做成“可解释、可回归”的最小集合：

- `FilesystemState`
  - `files: BTreeMap<String, FileRecord>`（key 使用绝对路径字符串，排序稳定）
- `FileRecord`
  - `content: Vec<String>`（按行；与 read_file 的分页输出语义一致）
  - `created_at: String`（ISO8601，若无法获取则可为空/省略，但需契约化）
  - `modified_at: String`（同上）
  - `deleted: bool`（删除标记；Phase 1 若不实现删除则明确为非目标并输出 not_supported）

说明：

- Phase 1 建议 **记录内容按行**（对齐 Python 版本“LLM 可编辑的 lines”体验），便于后续 patch/edit 与摘要策略
- 若担心大文件占用，可引入 `max_lines`/`max_bytes` 的策略，但需要可检测截断信号（见 2.5）

### 2.3 Delta 模型：中间件如何更新 state

建议采用显式 delta，便于 reducer 测试与可组合：

- `FilesystemDelta`
  - `files: BTreeMap<String, FileDelta>`
- `FileDelta`
  - `upsert: Option<FileRecordLike>`（新增/更新）
  - `delete: bool`（删除标记）

中间件职责：

- 将工具结果（write/edit/delete 等）转换为 delta
- 将 delta 交给 reducer 合并到 state

### 2.4 Reducer 合并规则（确定性要求）

需要写入契约并用单测锁定的规则：

- upsert 覆盖：同一路径的 upsert 覆盖旧 record（或按 modified_at 决胜，但必须确定）
- delete 优先级：
  - delete=true 必须使文件从 state 中移除或标记 deleted（两者二选一，需契约化）
  - delete 后的 upsert 行为必须定义（允许“复活”或拒绝；建议允许复活并视为新 record）
- 并发更新：
  - reducer 输入只接受“已排序/可比较”的 delta（BTreeMap），保证合并结果稳定
  - 若出现同一 file 多条 delta，合并顺序固定（例如按 delta 生成顺序或 modified_at）

### 2.5 Schema 固化策略（serde struct 优先）

Phase 1 的 schema 固化建议采用：

- **Rust struct（serde）作为唯一事实来源**
- 对外 JSON 兼容：通过 serde 兼容字段名与 default
- 回归方式（二选一，或同时）：
  - schema snapshot：将工具输入/输出的 JSON 样例与字段列表固化为文件，CI 中对比
  - schema tests：对每个工具的“缺字段/错类型/默认值”写单测与集成测试

必须明确并固化：

- `grep` 的 `output_mode`（files_with_matches/content/count）与 `head_limit` 语义
- `read_file` 的 `offset/limit` 默认值与 0-based/1-based 规则
- `execute` 的 timeout 单位与默认策略
- 错误码集合（file_not_found、parent_not_found、permission_denied、is_directory、invalid_path、file_exists、no_match、timeout、command_not_allowed、not_supported 等）

### 2.6 输出截断与预算（Phase 1 起必须可检测）

Phase 1 起要求“截断是可检测的”，否则 E2E 与后续 runtime 很难继续拉取：

- `read_file`：
  - 若因 `limit`、`max_bytes` 或内部预算截断，必须提供 `truncated=true` 或稳定提示字段
  - 继续拉取方式必须清晰：通过 offset/limit
- `execute`：
  - 若输出被截断，`ExecResult.truncated=true`
  - 超时返回 `timeout` 错误码（或同等可分类错误）

## 3. 实施顺序（里程碑拆解）

### M1：State 模型与 reducer（纯数据层）

- 任务
  - 定义 `FilesystemState`、`FilesystemDelta`、`FileRecord`、`FileDelta`
  - 定义 `StateReducer<FilesystemState>` trait
  - 实现默认 reducer，并写单测（覆盖覆盖/删除/并发更新）
- 验收
  - reducer 单测覆盖：覆盖覆盖、删除、并发更新三类核心场景
  - 数据结构 serde 序列化/反序列化稳定（用于 state-file）

### M2：Middleware 执行契约（trait 形态，最小可用）

- 任务
  - 将 `Middleware` 从空 trait 扩展为可执行契约（例如：`before_tool`/`after_tool` 钩子或统一 `handle`）
  - 定义 “工具调用上下文” 数据结构（tool name、input、output、error、root、当前 state）
  - 定义中间件链的组合方式（Vec 中间件顺序执行，顺序必须契约化）
- 验收
  - 可以在不改工具实现的情况下插入 FilesystemMiddleware
  - 中间件不会改变工具的业务语义，只负责 state 更新与审计

### M3：FilesystemMiddleware（把工具结果转成 delta）

- 任务
  - 为 `write_file/edit_file/(delete_file)` 等工具定义“如何更新 state”的规则
  - 将工具返回值（WriteResult/EditResult 等）映射为 delta
  - 对 `read_file/grep/glob/ls/execute` 等只读工具定义“不更新/只更新 metadata”的策略（Phase 1 可先不更新）
- 验收
  - 集成测试：write/edit → state 更新 → 再读取 state 可看到变更
  - 删除语义明确：支持 deletion marker 或明确 not_supported

### M4：State 可观测接口（CLI/E2E 的关键前置）

- 任务（推荐优先选择一种）
  - 方案 A：CLI `--state-file <path>`（最易落地）
  - 方案 B：CLI `session` 子命令（更完整，但实现更重）
  - 输出格式：每次 tool 调用输出 `{ output, delta?, state? }` 以便 E2E 断言
- 验收
  - Phase 1 E2E 可以通过该接口断言 state 的演进

### M5：Schema 回归与端到端测试

- 任务
  - 对每个工具补齐 schema 负向测试（缺字段/错类型/默认值）
  - 将 [E2E_PHASE1.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/E2E_PHASE1.md) 中的最小集合落地为可执行 smoke E2E
- 验收
  - `cargo test` 全量通过
  - 至少覆盖：state 写入、state 合并、分页继续读取、grep 结构化输出、错误码一致性

## 4. 测试计划（与验收强绑定）

### 4.1 单元测试（必须）

- Reducer
  - 覆盖：覆盖写入、删除、并发更新（同一路径多 delta）
- Schema
  - 覆盖：缺字段、错类型、默认值

### 4.2 集成测试（必须）

最小链路：

- tool 调用 → state 更新 → 后续 tool 使用该 state

建议至少覆盖：

- write_file → state.files 出现新条目
- edit_file → state.files 更新且 occurrences 正确
- read_file 分页 → 截断信号可检测（若启用预算）

### 4.3 E2E（建议强制）

执行基线参考：

- [E2E_PHASE1.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/E2E_PHASE1.md)

## 5. 风险与对策

- 风险：state 体积膨胀（大文件多）
  - 对策：Phase 1 引入 `max_lines/max_bytes` 与 truncation 信号；或只记录 metadata（但需在契约中明确）
- 风险：中间件执行契约不清导致后续 patch/memory 无法插入
  - 对策：Phase 1 优先把 Middleware trait 做成可组合的稳定面（Trait-first）
- 风险：schema 变更频繁导致上下游集成脆弱
  - 对策：schema snapshot + 负向测试双保险；字段改动必须伴随版本化策略说明

## 6. 输出物与文档索引

- 技术设计基线：[TECH_DESIGN.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/TECH_DESIGN.md)
- Phase 1 E2E 计划：[E2E_PHASE1.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/E2E_PHASE1.md)
- Phase 0 参考：
  - Phase 0 详细计划：[ITERATION_PHASE0_DETAILED.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/ITERATION_PHASE0_DETAILED.md)
  - Phase 0 E2E 计划：[E2E_PHASE0.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/E2E_PHASE0.md)
