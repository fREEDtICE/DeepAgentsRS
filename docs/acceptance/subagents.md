---
title: Core 验收 - Subagents（task 工具 / 上下文隔离 / 结果回传）
scope: core
---

## 1. 能力定义（E2E 效果）

Subagents 能力的端到端效果是：主 agent 能通过 `task` 工具启动短生命周期子 agent 来完成隔离任务，并且：

- 子 agent 的输入上下文与主线程严格隔离（不继承主 messages，不继承特定私有 state）
- 子 agent 可以使用自己的 middleware/tools/backend（与主线程共享或按 spec 指定）
- 子 agent 的输出以“单条结果”回传给主线程（只回传最后一条 message），避免污染主上下文
- 子 agent 的 state 回传遵循过滤规则（排除特定 keys）

参考 Python 实现： [subagents.py](../../../deepagents/libs/deepagents/deepagents/middleware/subagents.py)。

## 2. 对外契约（必须对齐）

### 2.1 `task` 工具 schema（LLM 可见）

LLM 可见参数只有两个：

- `description: string`
- `subagent_type: string`

其它运行期信息（tool_call_id、state、配置）必须由系统注入，不进入 schema。

### 2.2 parent → child 的 state 传递规则

必须对齐 Python 的 `_EXCLUDED_STATE_KEYS`（见 [subagents.py:L115-L128](../../../deepagents/libs/deepagents/deepagents/middleware/subagents.py#L115-L128)）：

- `messages`
- `todos`
- `structured_response`
- `skills_metadata`
- `memory_contents`

执行规则：

- child_state = parent_state 的浅拷贝（或序列化拷贝），但移除上述 keys
- 强制设置 `child_state.messages = [Human(description)]`

### 2.3 child → parent 的回传规则

- child 执行完成后必须产出 state，且 state 必须包含 `messages`
- parent 侧只取 child.messages 的最后一条 message 内容，包装成 ToolMessage(tool_call_id=...) 回注主 messages
- state_update 也必须过滤 `_EXCLUDED_STATE_KEYS`

## 3. 验收环境

- 主 runner 使用 ScriptedModel 驱动主线程
- 子 agent 同样可使用 ScriptedModel（可用“子脚本”）
- 注册至少两种 subagent_type：
  - `general-purpose`（默认存在）
  - `echo-subagent`（专门用于隔离断言：回显它看到了什么）

## 4. E2E 场景（Subagents 必测）

### SA-01：最小 task 闭环（子线程返回单条结果）

给定：

- 主模型输出 tool_call：task(description="say hi", subagent_type="general-purpose")
- general-purpose 子 agent 的脚本：返回 Assistant("HI") 并终止

当：运行主 Runner

则：

- 主线程出现 ToolMessage(content="HI", tool_call_id=...)（或等价内容）
- 主线程可继续下一轮并最终收敛

### SA-02：隔离：child messages 仅包含任务描述

给定：

- 主 state.messages 包含多轮历史（含敏感占位文本 "SECRET_IN_MAIN"）
- 子 agent 的脚本会断言其输入 messages：
  - 必须只有 1 条 HumanMessage，内容为 description
  - 不允许出现 "SECRET_IN_MAIN"

当：主模型调用 task(description="check isolation", subagent_type="echo-subagent")

则：

- 断言成立（若不成立，子 agent 返回 error，场景应失败）

### SA-03：隔离：excluded keys 不下发

给定：

- parent_state 设置：
  - skills_metadata={"k":"v"}
  - memory_contents="MEM"
  - todos=[...]
- 子 agent 会回显其看到的 state keys 列表

当：主模型调用 task(...)

则：

- 子 agent 输出中不包含 skills_metadata/memory_contents/todos/messages/structured_response
- 允许出现其它非敏感 keys（例如 backend 相关配置、thread_id 等，取决于实现）

### SA-04：回传：只回传最后一条 message

给定：

- 子 agent 脚本产生多条 assistant message：
  - Assistant("step1")
  - Assistant("step2")
  - Assistant("final")

当：主模型调用 task(...)

则：

- 主线程收到的 ToolMessage 只能是 "final"
- 主线程 messages 中不应出现 step1/step2（避免子线程过程污染）

### SA-05：回传：state_update 过滤规则

给定：

- 子 agent 执行后返回 state，其中包含：
  - allowed_key={"x":1}
  - todos=[...]（被排除）
  - messages=[...]（被特殊处理：只回传最后一句为 ToolMessage）

当：主模型调用 task(...)

则：

- parent_state.allowed_key 被合并（或按合并策略更新）
- parent_state.todos 不被子线程覆盖或注入

### SA-06：CompiledSubAgent 的输出必须包含 messages

给定：

- 注册一个 CompiledSubAgent，其 runnable 返回的 state 不包含 messages
- 主模型调用 task(subagent_type="broken-compiled")

当：执行 task

则：

- task 返回明确错误（指出 subagent 输出 schema/结果缺少 messages）
- 主线程不崩溃，可继续下一轮

### SA-07：子线程可再调用工具，但不泄漏到主线程历史

给定：

- 子 agent 拥有 Filesystem 工具并在子线程内写文件 `/child.txt`
- 子 agent 最终输出 "DONE"

当：主模型调用 task(...)

则：

- `/child.txt` 的副作用发生在共享 backend 中（如果主/子共享 backend），可在 workspace 找到
- 主线程 messages 不包含子线程内部的 write_file/edit_file 过程，只包含 ToolMessage("DONE")

### SA-08：嵌套 task（子线程内部再 spawn subagent）

给定：

- 子 agent 在其脚本中发起 task 调用（嵌套）

当：执行主模型 task

则：

- 系统要么明确支持嵌套并正确隔离每层 state/messages，要么明确拒绝嵌套并给出可诊断错误
- 行为必须固定（避免某些运行时支持、某些运行时崩溃）

## 5. 通过标准

- SA-01 ~ SA-08 全通过
- 每个场景都能从 events.jsonl 断言：
  - 子线程启动/结束边界
  - tool_call_id 对齐
  - 主线程只接收到最后一条结果

