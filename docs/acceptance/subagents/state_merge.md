---
title: Core Subagents E2E - 回传与 state 合并（只回最后一句）
scope: core
---

## 1. 端到端效果（主线程的收敛与可控合并）

子 agent 往往会做多步推理与多次工具调用。端到端必须保证：

- 主线程只拿到子线程“最终结果”，避免把过程污染主上下文
- 子线程的 state 回传必须过滤私有/无 reducer keys

对齐 Python 回传规则（证据）： [subagents.py](../../../../deepagents/libs/deepagents/deepagents/middleware/subagents.py)：

- 只回传 child.messages 最后一条
- state_update 过滤 messages/todos/structured_response/skills_metadata/memory_contents

## 2. 验收环境

- 子 agent 使用脚本产生多条 assistant message，并返回额外 state keys
- 主 Runner 记录 events 与 final_state

## 3. E2E 场景（必测）

### SM-01：只回传最后一条 message

给定：

- 子 agent 输出 messages：
  - Assistant("step1")
  - Assistant("step2")
  - Assistant("final")

当：主模型调用 task(...)

则：

- 主线程只回注 ToolMessage("final")
- 主线程 messages 中不出现 step1/step2

### SM-02：ToolMessage 必须带 tool_call_id 对齐

给定：

- 主模型发起 task tool_call_id="t123"

当：子线程完成回传

则：

- ToolMessage.tool_call_id == "t123"

### SM-03：state_update 过滤 excluded keys

给定：

- 子 agent 返回 state：
  - allowed={"x":1}
  - todos=[{...}]
  - memory_contents="LEAK"

当：合并回主线程

则：

- final_state.allowed 被更新/合并
- final_state.todos 不因子线程变化
- final_state.memory_contents 不被覆盖/注入

### SM-04：允许回传的 state 合并策略必须固定

给定：

- 主 state.allowed={"a":1}
- 子 state.allowed={"b":2}

当：回传

则（二选一，必须固定并文档化）：

- 方案 A：深合并得到 {"a":1,"b":2}
- 方案 B：覆盖得到 {"b":2}

### SM-05：子线程内部文件副作用与主线程消息隔离同时成立

给定：

- 子 agent 在子线程内执行 write_file("/child.txt","x")
- 子 agent 最终输出 "DONE"

当：主线程调用 task(...)

则：

- backend 中确实存在 /child.txt（副作用发生）
- 主线程 messages 不包含子线程工具过程，只包含 ToolMessage("DONE")

### SM-06：子线程异常的传播语义必须可诊断

给定：

- 子 agent 运行时 panic/返回错误

当：主线程调用 task(...)

则：

- task tool 返回 error ToolMessage（含可诊断原因）
- 主线程不崩溃，可继续下一轮

## 4. 通过标准

- SM-01 ~ SM-06 全通过

