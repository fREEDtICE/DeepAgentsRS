---
title: 技术方案与迭代计划 - HITL 会话级 interrupt/resume（CLI + ACP）
scope: iteration
---

## 1. 问题背景

Rust 版当前在需要人工审批（HITL, Human-in-the-loop）时没有形成“暂停→决策→续跑”的闭环：当 `execute` 等危险工具触发 `RequireApproval`，系统只会把它当作一次工具错误写回（并提示“需要交互审批，但尚不支持”），随后继续推进或直接失败。这会导致：

- 上层（CLI/ACP）无法在同一会话中接管并驱动后续执行
- 无法满足“tool_call_id 对齐、不重复执行、可 reject/edit”的端到端语义
- “需要审批”退化为不可恢复错误，无法形成可测试的门禁

相关现状入口：

- Rust runtime 目前在 `RequireApproval` 时写入工具错误而非暂停：[simple.rs](../../crates/deepagents/src/runtime/simple.rs#L751-L795)
- Rust HTTP ACP 也以工具错误返回（缺少 pending/resume API）：[server.rs](../../crates/deepagents-acp/src/server.rs#L250-L302)
- 期望的对外契约与 E2E 场景见验收文档：[hitl.md](../acceptance/hitl.md)
- Python CLI 已有可参考的 interrupt/resume 闭环（LangGraph）：[textual_adapter.py](../../../deepagents/libs/cli/deepagents_cli/textual_adapter.py#L1077-L1214)

## 2. 目标与非目标

### 2.1 目标（必须满足）

- **会话级暂停/续跑**：同一 `session` 内可多次 interrupt，并可在后续请求中 resume。
- **三类决策**：approve / reject / edit（修改 args）。
- **幂等与对齐**：
  - tool_call_id 全程对齐
  - 同一个 interrupt 只能导致工具“执行一次或被取消一次”，不得重复执行
- **覆盖 CLI 与 ACP**：
  - CLI：交互式模式可直接在本地驱动决策并续跑
  - ACP：提供可被远端客户端驱动的 pending 查询与 resume 接口
- **错误语义**：resume 载荷无效时返回明确错误且不丢失 pending interrupt（允许重试）。

### 2.2 非目标（本迭代不做）

- 不做 Textual/TUI 级审批 UI 复刻（先提供最小 CLI 交互）。
- 不在第一阶段强制落盘保存完整消息历史（先保证会话内闭环；落盘作为后续增强）。
- 不引入新的 provider/协议大版本（优先在 v1 增量字段兼容）。

## 3. 核心设计（建议采取的架构）

### 3.1 把 HITL 视为“Runner 能力”而非“工具错误”

HITL 的关键不是“拒绝某次工具调用”，而是“在工具执行边界暂停，并允许上层提交决策再继续”。因此需要把 runtime 的执行过程拆成可暂停的“步进式 Runner”，并把 pending interrupt 明确建模出来。

推荐引入一个可恢复的执行器（可命名为 `ResumableRunner`），它管理三类状态：

- `messages: Vec<Message>`：会话内的消息历史（包含 tool_calls 与 tool 结果）
- `state: AgentState`：中间件/工具状态（已有）
- `cursor`（可选）：表示“当前正处于 provider step 之后、工具批处理中第几个 call”

理由：

- 仅靠 `AgentState` 无法重建“未执行的 tool_call 在消息流中的位置”，而对齐 `tool_call_id` 与“不重复执行”需要可重放的上下文。
- Runner 负责“暂停/续跑”，runtime 负责“策略 + 执行”，职责分层更清晰；也更容易把同一能力暴露给 CLI 与 ACP。

### 3.2 协议与数据模型（对外契约）

对外应固定以下三类对象（字段名可微调，但必须文档化并有 E2E 断言）：

#### Interrupt 事件

```json
{
  "interrupt_id": "i-<stable>",
  "tool_name": "write_file",
  "tool_call_id": "call_123",
  "proposed_args": { "...": "..." },
  "policy": {
    "allow_approve": true,
    "allow_reject": true,
    "allow_edit": true
  },
  "hints": {
    "display": "可选：给 UI/CLI 的提示文案",
    "danger_level": "可选：low/medium/high"
  }
}
```

- `interrupt_id` 必须在会话内稳定，用于 resume 时指向同一 pending。
- 推荐将 `interrupt_id = tool_call_id`（最简单且满足稳定性）；若 provider 不提供 call_id，则使用 runtime 的 `next_call_id` 生成并保存。

#### Resume 载荷（上层输入）

```json
{ "type": "approve" }
```

```json
{ "type": "reject", "reason": "optional" }
```

```json
{ "type": "edit", "args": { "...new args..." } }
```

#### Run 输出状态

在现有 `RunOutput` 的基础上增加**兼容性字段**，避免破坏已有调用方：

```json
{
  "status": "completed | interrupted | error",
  "interrupts": [ /* optional, non-empty when interrupted */ ],
  "final_text": "...",
  "tool_calls": [ ... ],
  "tool_results": [ ... ],
  "state": { ... },
  "error": null
}
```

- 若 `interrupts` 非空，则 `status=interrupted`；此时 `error` 必须为 null（避免把可恢复状态误判成错误）。
- 现有字段保持不变，旧客户端可以忽略新增字段（兼容）。

### 3.3 Pending 状态（会话内存储）

当 runner 发现需要 HITL 的工具调用时：

- **不执行工具**
- 在会话内存储一个 `PendingInterrupt`，至少包含：
  - `interrupt: HitlInterrupt`
  - `pending_call: ProviderToolCall`（原始 call，包括 call_id）
  - `remaining_calls: Vec<ProviderToolCall>`（同一批次余下 call，用于 H-04 的“连续 interrupt”语义）
  - `messages_checkpoint_len: usize`（可选：用于回滚或断言不重复写入）

存储位置建议：

- ACP：放在 `Session` struct 中（内存 session），并在 `/session_state` 暴露 `pending_interrupt` 摘要。
- CLI：交互式直接放进进程内的 runner；非交互式可选择把 pending 编入输出 JSON，供外部驱动 resume（后续增强可落盘）。

### 3.4 触发条件（interrupt_on + approval）

触发 HITL 的来源至少两类：

1) **策略式 interrupt_on**（面向通用工具）：例如 write_file/edit_file/delete_file/execute 等  
2) **审批策略 RequireApproval**（面向 execute 的 allow-list 审批）：当 `ApprovalPolicy.decide(...)` 返回 `RequireApproval` 时必须 interrupt

建议统一为：在执行工具前做一次 `should_interrupt(call, state, policy)` 判定，命中则生成 Interrupt。

### 3.5 决策应用（approve/reject/edit）

resume 后 runner 需按决策应用到同一个 `pending_call` 上，并保证 tool_call_id 不变：

- approve：按原 args 执行，并写入正常 ToolMessage（success/error）
- edit：用新 args 执行，但 ToolMessage 必须仍使用原 `tool_call_id`；建议在 output 中增加可诊断字段（例如 `{"edited":true,"original_args":...}` 的机器字段，模型可见字段保持简洁）
- reject：不执行工具，直接注入一条 ToolMessage，语义上等价于“已取消且无副作用”；建议 `status="rejected"` 并带可选 `reason`

### 3.6 无效 resume 的错误语义

若决策载荷无效（例如 edit 缺少必填字段）：

- 返回 `status=error`，并附带结构化错误（例如 `error.code="invalid_resume"`）
- **保持 pending interrupt 不变**（仍可再次 resume）
- **不得执行工具**（副作用为零）

## 4. ACP（HTTP v1）接口扩展建议

现状 ACP v1 只有 `/run` 与 `/call_tool`，缺少 pending/resume 能力。建议在不升级大版本的前提下做增量扩展：

- `POST /run`：执行直到完成或遇到 interrupt；返回带 `status` 的 `RunOutput`。
- `POST /resume`（新增）：输入 `session_id + interrupt_id + decision`，继续执行直到完成或下一个 interrupt。
- `GET /session_state/:id`：增加可选字段 `pending_interrupt`（摘要）。

建议请求体：

```json
{
  "protocol_version": "v1",
  "session_id": "...",
  "interrupt_id": "...",
  "decision": { "type": "approve" }
}
```

兼容策略：

- 旧客户端不调用 `/resume` 时，若会话处于 pending，则 `/run` 直接返回同一个 interrupt（幂等）。
- `/call_tool` 与 `/run` 的 `state_version` 语义保持：每次产生新可观察状态变化（包括进入 pending）都递增。

## 5. CLI（Rust）交互建议

在 Rust CLI 侧提供最小可用交互（不依赖 TUI）：

- `--execution-mode interactive` 时：
  - run 过程中若遇到 interrupt，打印待审批工具（tool_name、args 摘要）
  - 读取 stdin 决策：`a=approve / r=reject / e=edit`
  - edit 可要求用户输入一段 JSON（新 args），或使用 `$EDITOR`（后续增强）
  - 自动调用 resume 并继续，直到完成或 max_steps
- `--execution-mode non_interactive` 时：
  - 遇到 interrupt 直接输出 JSON（包含 interrupt），并退出为可判定的退出码（例如 2），供外部 orchestrator 驱动 `/resume` 或再次 run

## 6. 测试与验收门禁（对齐 docs/acceptance/hitl.md）

以 [hitl.md](../acceptance/hitl.md) 的 H-01 ~ H-05 作为强制门禁，建议将其落到 Rust e2e test（workspace 级）：

- H-01 approve：write/edit 文件产生副作用
- H-02 reject：不产生副作用且注入 rejected ToolMessage
- H-03 edit：修改参数后执行且 tool_call_id 不变
- H-04 连续 interrupt：同一轮多个 tool_calls 逐个暂停与续跑
- H-05 无效 resume：返回错误且 pending 不丢失，可重试

## 7. 迭代拆分（建议按依赖顺序）

### I4-1：定义协议与状态结构（纯结构，不改行为）

- 交付物
  - `HitlInterrupt/HitlDecision/RunStatus` 的 Rust 数据结构与序列化
  - `RunOutput.status/interrupts` 兼容性字段（默认不启用）
  - ACP `/session_state` 增加 `pending_interrupt` 字段（先可为空）
- 验收
  - `cargo test --workspace` 通过
  - JSON 序列化快照测试（字段名固定）

### I4-2：ResumableRunner 最小闭环（仅内存，会话级）

- 交付物
  - 新增 `ResumableRunner`（或等价实现）：支持 interrupt → resume（approve/reject/edit）
  - 先覆盖 `interrupt_on`（write_file/edit_file/delete_file/execute），并把 `RequireApproval` 映射为 interrupt
- 验收
  - H-01/H-02/H-03 通过（可先不接 ACP/CLI UI）

### I4-3：Rust CLI 接入（交互闭环）

- 交付物
  - CLI 在 interactive 模式遇到 interrupt 能完成 prompt 并自动续跑
  - non_interactive 模式返回可机器消费的 interrupted 输出
- 验收
  - CLI 端到端脚本可复现 H-01~H-03

### I4-4：Rust HTTP ACP 接入（远端驱动闭环）

- 交付物
  - `POST /resume` 端点
  - `/run` 在 pending 时幂等返回同一 interrupt
  - session 内保存 `messages/state/pending`，确保多次 interrupt 可续跑
- 验收
  - 针对 ACP 的 e2e：H-01~H-05 全通过（覆盖连续 interrupt 与无效 resume）

### I4-5：一致性与可观察性打磨

- 交付物
  - audit 事件补齐：allow/deny/require_approval/reject/edit（含必要脱敏）
  - 错误码统一：`invalid_resume / hitl_pending / interrupt_not_found` 等
  - 文档补齐：对齐本文件与 [hitl.md](../acceptance/hitl.md)
- 验收
  - CI 门禁稳定（Rust CLI 与 ACP 各至少一条黑盒链路）

## 8. 风险与权衡

- **消息历史存储**：会话级 resume 需要保存 messages；若无限增长需依赖 summarization/offload（后续与 I2/I3 联动）。
- **兼容性**：在 `RunOutput` 增量字段而不破坏既有 JSON 解析；ACP v1 端点新增保持向后兼容。
- **并行 tool_calls**：当前 runtime 顺序执行；HITL 语义也要求按序逐个暂停，避免并行工具引入不确定性。

