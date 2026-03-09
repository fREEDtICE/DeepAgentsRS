---
title: Core Todo E2E - 并行调用防线（同轮多 write_todos）
scope: core
---

## 1. 端到端效果

同一轮 assistant message 中出现多个 write_todos tool_calls 时，系统必须：

- 拒绝该轮的所有 write_todos 调用
- 不更新 state.todos
- 返回明确可诊断错误（ToolMessage error）

这是执行层硬约束，不依赖 prompt 自律。Python 证据： [test_todo_middleware.py](../../../../deepagents/libs/deepagents/tests/unit_tests/test_todo_middleware.py)。

## 2. 验收环境

- ScriptedModel 产出单条 assistant message，包含多个 write_todos tool_calls
- 初始 state.todos 预置非空，以便断言“没有被改写”

## 3. E2E 场景（必测）

### TP-01：同轮 2 次 write_todos 全部拒绝

给定：

- 初始 todos=[{id:"a",...}]
- assistant tool_calls：
  - write_todos(id=x, merge=false, todos=[{id:"b",...}])
  - write_todos(id=y, merge=true, todos=[{id:"a",status:"completed"}])

当：运行 Runner

则：

- 两个 tool_call 都产生 error ToolMessage（拒绝原因可判定）
- final_state.todos 仍为初始值（a 未变化，b 未加入）

### TP-02：同轮 3 次 write_todos 仍全部拒绝

给定：

- 3 个 write_todos tool_calls

当：运行 Runner

则：

- 3 个都被拒绝
- state.todos 不变

### TP-03：write_todos 与其它工具并存时，只拒绝 write_todos

给定：

- 同轮 tool_calls：
  - write_todos(...)
  - echo_tool(...)
  - write_todos(...)

当：运行 Runner

则：

- 两个 write_todos 都拒绝
- echo_tool 仍可正常执行（除非系统有“同轮多工具”禁用策略，但那必须单独文档化）

## 4. 通过标准

- TP-01 ~ TP-03 全通过

