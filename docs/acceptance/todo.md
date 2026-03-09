---
title: Core 验收 - Todo（write_todos 工具 / state.todos 语义）
scope: core
---

## 1. 能力定义（E2E 效果）

Todo 能力的端到端效果是：Agent 能通过 `write_todos` 工具维护一个结构化 todo 列表，并且：

- tool schema 稳定（todos 数组、merge 布尔、可选 summary）
- state.todos 被更新为可机器消费的结构化数据
- 同一轮 assistant message 中并行多次 write_todos 必须被拒绝（硬约束）
- write_todos 的更新必须在事件流与最终 state 中可观测、可断言

参考 Python 约束证据：

- 并行调用拒绝测试： [test_todo_middleware.py](../../../deepagents/libs/deepagents/tests/unit_tests/test_todo_middleware.py)
- prompt 快照（强调不并行调用）： [system_prompt_without_execute.md](../../../deepagents/libs/deepagents/tests/unit_tests/smoke_tests/snapshots/system_prompt_without_execute.md)

## 2. 工具契约（建议对齐）

本项目 Rust Core 建议采用与现有 IDE 工具一致的 schema（便于复用）：

- `todos: [{id, content, status, priority}]`
- `merge: bool`
- `summary?: string`（仅在有任务从未完成变为完成时允许）

关键约束：

- `write_todos` 调用成功后，state.todos 必须等于更新后的完整 todo 列表（而不是增量）
- 不允许在同一轮 assistant 输出中出现多个 write_todos（见 4.4）

## 3. 验收环境

- 使用 ScriptedModel 驱动，直接产出 write_todos tool_call
- Runner 必须把 write_todos 的 ToolMessage 与 state 更新记录到 events.jsonl

## 4. E2E 场景（Todo 必测）

### TD-01：首次写入 todo（replace 语义）

给定：

- 初始 state.todos 为空
- 模型调用 write_todos：
  - merge=false
  - todos=[{id:"a",content:"A",status:"pending",priority:"high"}]

当：执行工具

则：

- final_state.todos 精确等于给定数组（仅包含 a）
- 事件流包含 StateUpdated(todos) 与 ToolMessage(success)

### TD-02：merge=true 的按 id 合并语义

给定：

- 初始 state.todos：
  - [{id:"a",content:"A",status:"pending",priority:"high"}]
- 模型调用 write_todos：
  - merge=true
  - todos=[{id:"a",status:"completed"}]

当：执行工具

则：

- final_state.todos 中仍包含 id="a"
- 该项的 status 变为 completed
- 未提供的字段保持不变（content/priority 不被清空）

判定重点：merge 是“按 id 字段局部覆盖”，不是整项替换。

### TD-03：merge=false 的完全替换语义

给定：

- 初始 state.todos 有多项
- 模型调用 write_todos(merge=false, todos=[...仅 1 项...])

当：执行工具

则：

- final_state.todos 只剩该 1 项

### TD-04：同轮并行多次 write_todos 必须全部拒绝

给定：

- ScriptedModel 在同一条 assistant message 中输出两个 tool_calls：
  - write_todos(merge=false, todos=[...])
  - write_todos(merge=true, todos=[...])

当：Runner 执行工具分发

则：

- 两个 tool_call 都返回 error ToolMessage（错误文案可不同，但必须可判定为拒绝原因）
- final_state.todos 不发生任何变化（保持初始值）
- events 中能定位到拒绝原因（例如 "write_todos should not be called multiple times in parallel"）

判定重点：这是执行层硬约束，不依赖 prompt 自律。

### TD-05：summary 字段门禁（仅在发生完成态转移时允许）

给定：

- 初始 state.todos：
  - [{id:"a",status:"pending",...}]
- Case A：write_todos(merge=true, todos=[{id:"a",status:"completed"}], summary="done")
- Case B：write_todos(merge=true, todos=[{id:"a",status:"pending"}], summary="should_fail")

当：分别执行

则：

- Case A：成功，final_state.todos[a].status==completed，ToolMessage(success)
- Case B：失败或忽略 summary，必须给出明确可诊断行为（两者择一但要固定）

判定重点：summary 不是任意可写字段，需要有门禁，否则会污染对话与 UI。

### TD-06：id 唯一性与错误处理

给定：

- write_todos(merge=false, todos=[{id:"a"...},{id:"a"...}])

当：执行

则：

- 返回明确错误（重复 id）
- state.todos 不变

## 5. 通过标准

- TD-01 ~ TD-06 全通过
- 任一失败都能从 events.jsonl 定位到 tool_call_id、拒绝原因与 state 是否被更新

