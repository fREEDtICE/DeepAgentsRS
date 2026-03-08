# Phase 3 详细迭代计划（ACP server：端到端会话与工具调用）

适用范围：本计划面向 [ITERATION_PLAN.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/ITERATION_PLAN.md#L121-L150) 的 Phase 3。目标是提供可用的 ACP 服务端：会话、消息、工具调用、结果回传；并复用 Phase 1/2 的 tool schema、错误码语义与 execute 安全策略。

本计划的核心原则：

- 先定外部契约：先把“服务端对外能做什么、输入输出长什么样”固定下来，E2E 才能落地
- 先闭环后扩展：Phase 3 只做最小可用闭环；流式、长连接、鉴权、限流等作为后续阶段扩展
- 分层不打架：传输层（HTTP/stdio）与业务层（会话/工具/状态）隔离，便于替换
- Trait 优先：会话存储、审计、审批/策略、传输层都必须可插拔（第三方无需 fork）<mccoremem id="03fpi56spbrjzzuvwcz8hsm8u" />

## 1. 完成定义（Definition of Done）

Phase 3 完成必须同时满足：

- 服务端可运行：可在本地/CI 启动并对外提供 API
- 会话闭环：建立会话 → 会话内调用工具 → 返回结构化结果 → 关闭会话
- 复用 Phase 1/2 契约：
  - tool 输入 schema 严格校验（缺字段/错类型/未知字段等行为稳定）
  - 错误码语义稳定（file_not_found/is_directory/no_match/timeout/command_not_allowed 等）
  - execute 非交互 deny-by-default + allow-list + 危险模式拒绝 + 审计/脱敏（Phase 2）
- 黑盒 E2E 落地：以第三者视角驱动服务端进程并跑通测试计划（见 [E2E_PHASE3_ACP.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/E2E_PHASE3_ACP.md)）

## 2. 当前系统情况（Phase 3 基线）

### 2.1 已有可复用的业务内核

- 工具与状态回填链路已具备：可作为 ACP “tool 调用执行内核”
  - `DeepAgent::call_tool_stateful(...)` 支持在给定 `AgentState` 上执行工具并产出 delta（filesystem）：用于会话内 state 演进
- Phase 2 的执行安全与审计已落地（供 Phase 3 复用）：非交互 deny-by-default、allow-list、危险模式分类码、审计 JSONL 与脱敏

### 2.2 现有 ACP server（Rust）缺口

- `deepagents-acp` crate 目前只有启动骨架与 `--bind` 参数，占位但未提供任何网络接口/协议/路由
- 因此 Phase 3 的第一优先级是：选定可测的对外协议并补齐最小端到端闭环

## 3. 协议与对外契约（必须先冻结，E2E 才能落地）

Phase 3 不要求与任何第三方 ACP 传输实现“字节级一致”，但必须满足 E2E 所需的最小能力点（创建会话、工具调用、读取 state、关闭会话）。为保证可测与可演进，本计划选择“默认协议 + 可替换传输层”的方式：

### 3.1 默认传输选型（Phase 3 默认实现）

- HTTP + JSON（请求/响应）
  - 优点：最易写黑盒 E2E（CI/本地一致）、最容易做错误码与结构化输出契约
  - 缺点：与某些 ACP 生态（若偏 JSON-RPC/stdio）可能存在适配成本

备注：若未来需要 stdio/JSON-RPC（例如嵌入式/CLI 集成），要求复用同一业务层 handler，仅替换 transport。

### 3.2 版本与兼容策略

- 所有请求与响应必须携带 `protocol_version`（或在顶层 `initialize` 中返回并冻结），Phase 3 初始为 `v1`
- 兼容规则：
  - 新增字段允许（必须为可选字段，且旧客户端可忽略）
  - 破坏性变更必须通过新版本号引入（Phase 3 不做）

### 3.3 方法集（Phase 3 最小子集）

无论底层协议如何封装，服务端必须提供等价能力：

1) `initialize`（可选但建议）
- 输入：客户端信息（可空）
- 输出：server 信息（name/version/protocol_version），以及能力声明（支持哪些工具、是否支持 state 查询）

2) `new_session`
- 输入（最小）：`root`（或 server-side profile id）
- 可选输入：`execution_mode`、`shell_allow_list`、`audit_json`（是否启用审计由 server 决定，但至少要支持“server-side 审计开关”）
- 输出：`session_id`

3) `call_tool`
- 输入：`session_id`、`tool_name`、`input`（JSON）
- 输出：结构化结果（见 3.4）

4) `get_session_state`（强烈建议）
- 输入：`session_id`
- 输出：完整 state（至少 filesystem 维度）

5) `end_session`
- 输入：`session_id`
- 输出：成功/失败（幂等）

备注：Phase 3 可以暂不提供“prompt/对话”能力。Phase 3 的验收只要求“通过 ACP 调用工具并返回结果”，而不是完整 agent runtime。

### 3.4 统一响应结构（黑盒可断言）

所有方法输出都必须是 JSON，并遵循：

- 成功：
  - `ok: true`
  - `result: <method-specific>`
- 失败：
  - `ok: false`
  - `error: { code: string, message: string, details?: any }`

`call_tool` 的 `result` 必须至少包含：

- `output: any|null`
- `error: { code: string, message: string } | null`
  - 注意：这里的 `error` 是“工具级错误”（例如 file_not_found），与顶层 `ok=false` 的“协议级错误”（例如 session_not_found）区分
- `state?: AgentState` 或 `delta?: FilesystemDelta`
  - 至少必须有一个可用于验证 state 演进（建议两者都返回）
- `state_version: number`（强烈建议）
  - 递增版本号，便于并发与回归断言

## 4. 核心设计（Trait-first，避免绑死 Web 框架）

### 4.1 分层与职责

- Transport 层：负责监听/解析/序列化（HTTP/stdio/WebSocket 等）
- RPC/Handler 层：把“方法名 + JSON 入参”映射到业务动作（initialize/new_session/call_tool/...）
- Session 层：维护会话生命周期与并发隔离（state/version/root/policy/audit）
- Execution 内核：调用 deepagents 工具（复用 Phase 1/2 的 schema 与错误码）

### 4.2 关键 trait（Phase 3 需要先定接口）

1) `SessionStore`
- `create(root, config) -> session_id`
- `get(session_id) -> SessionHandle`
- `update(session_id, f: FnOnce(&mut Session))`
- `end(session_id)`
- 约束：必须支持并发安全与最小资源回收（至少 end_session 后释放）

2) `AcpHandler`（或等价）
- `initialize(req) -> resp`
- `new_session(req) -> resp`
- `call_tool(req) -> resp`
- `get_session_state(req) -> resp`
- `end_session(req) -> resp`

3) Phase 2 复用 trait
- `ApprovalPolicy`：对 execute 做策略决策（deny/allow/require_approval + 分类码）
- `AuditSink`：将 allow/deny/执行完成记录为 JSONL（默认脱敏）

### 4.3 会话模型（最小必要字段）

每个 session 至少包含：

- `session_id`
- `root`
- `state: AgentState`
- `state_version: u64`
- `execution_mode`（默认 non_interactive）
- `approval_policy`（可替换）
- `audit_sink`（可选）
- `created_at/last_accessed_at`（用于 TTL 回收，可选但建议预留）

并发策略（建议）：

- 每个 session 一个锁（Mutex/RwLock），保证同一 session 的 state_version 单调递增
- 不同 session 可并发执行（避免全局锁）

### 4.4 错误模型与映射

必须区分：

- 协议/会话级错误（顶层 `ok=false`）
  - `invalid_request`（非法 JSON/缺字段/未知方法）
  - `session_not_found`
  - `already_closed`
- 工具级错误（`call_tool.result.error`）
  - 复用 Phase 1/2 错误码语义（file_not_found/is_directory/no_match/timeout/command_not_allowed 等）
  - execute 被策略拒绝时：
    - `code` 为 `approval_required|dangerous_pattern|not_in_allow_list|empty_command|unknown`
    - `message` 给出可诊断原因，但不得包含敏感信息

## 5. 安全与审计（Phase 2 复用到 ACP）

Phase 3 必须确保：任何能触发 `execute` 的路径都必须走 Phase 2 的策略与审计（不能因为 ACP 入口而绕过）。

要求：

- session 创建时固定 `execution_mode`（默认 non_interactive）
- `call_tool(tool_name="execute")` 必须在执行前经过 `ApprovalPolicy`
- allow-list 来源：
  - 可从 `new_session` 参数传入（推荐：list of program names）
  - 或服务端配置 profile（用于生产）
- 审计：
  - server-side 写入 JSONL（路径由 server 配置或 session 参数指定，具体策略由产品决定）
  - 必须脱敏（command_redacted），不得记录原始 secret

## 6. 迭代拆解（里程碑）

### M0：冻结对外契约与 E2E Harness 约定

- 输出
  - 本文档冻结方法集与统一响应结构
  - 更新/补齐 Phase 3 黑盒 E2E 测试计划中的“最小契约点”（若未包含 state_version 等）
- 验收
  - 可据此编写 E2E harness（不依赖实现细节）

### M1：ACP server 最小可运行（仅会话与 health）

- 任务
  - deepagents-acp 可启动并监听
  - 提供 `initialize` 与 `new_session/end_session`
  - 内存 SessionStore（HashMap + per-session lock）
- 验收
  - E2E：启动、建会话、关会话幂等

### M2：会话内工具调用闭环（read_file/grep/execute）

- 任务
  - `call_tool` 接入 deepagents 工具执行内核
  - 返回 `output/error/state/delta/state_version`（至少满足 3.4）
  - `get_session_state`
- 验收
  - E2E：read_file/grep 成功路径 + 常见错误码

### M3：复用 Phase 2 策略与审计（ACP 入口不绕过）

- 任务
  - `execute` 在 ACP 入口同样 enforce deny-by-default/allow-list/危险模式分类码
  - 审计 JSONL：允许/拒绝/执行完成均记录
- 验收
  - E2E：deny-by-default、危险模式拒绝、allow-list 放行、审计脱敏

### M4：并发与资源回收（最小可靠性）

- 任务
  - 多会话并发：不会互相污染
  - 同会话串行保证：state_version 单调递增
  - session 资源释放：end_session 后不可再调用；可选 TTL 回收
- 验收
  - E2E：并发建会话 + 并发调用；关闭后拒绝

## 7. 测试计划（Phase 3 的第一优先级）

- 黑盒 E2E：以第三者视角驱动 server（见 [E2E_PHASE3_ACP.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/E2E_PHASE3_ACP.md)）
- 单元测试（建议但不替代 E2E）
  - SessionStore 并发与幂等
  - 请求解析与错误映射（invalid_request/session_not_found）

## 8. 非目标（Phase 3 不做）

- 鉴权/认证、TLS、生产级限流与配额
- 流式响应、长连接会话推送（SSE/WebSocket）
- 完整 agent runtime（prompt 驱动多轮 tool-calling）

