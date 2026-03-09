---
title: Core Runner E2E - 事件流与可断言性
scope: core
---

## 1. 为什么事件流是 Core 的验收对象

Core 的 E2E 验收并不只关心“最终输出对不对”，还要关心：

- 发生了哪些工具调用、副作用在哪里、顺序是否正确
- 为何终止（收敛/中断/错误/上限），能否诊断
- 上层（CLI/TUI/服务）如何以稳定接口消费执行过程

因此 Runner 必须提供结构化事件流。

## 2. 事件模型（最小可用集合）

事件类型不要求与 Python 一致，但必须覆盖等价信息。最低集合：

- `ModelRequestBuilt`：轮次号、messages 摘要、tools 名称集合、system 摘要
- `AssistantMessage`：完整 assistant message（含 tool_calls）
- `ToolCallStarted`：tool_name、tool_call_id、args 摘要
- `ToolCallFinished`：tool_name、tool_call_id、result 摘要或 error
- `ToolMessageAppended`：tool_call_id、content 摘要
- `StateUpdated`：更新的 key 列表（可选带 patch diff）
- `Interrupt`：tool_name、tool_call_id、proposed_args、policy
- `RunFinished`：终止原因与统计（轮次数、工具次数、错误次数）

## 3. 事件流的稳定性约束（必须）

### 3.1 顺序约束

同一轮内事件顺序必须固定：

1) ModelRequestBuilt
2) AssistantMessage
3) 对每个 tool_call：
   - ToolCallStarted
   - ToolCallFinished
   - ToolMessageAppended
   - StateUpdated（如果该工具产生 update）
4) RunFinished（若该轮收敛或终止）

### 3.2 可复现性约束

在 ScriptedModel 下，同一输入必须产生完全一致的：

- events 序列长度
- 事件类型顺序
- 关键字段：轮次、tool_name、tool_call_id、终止原因

允许变化的只有与时间有关的字段（若存在），但建议 Core 不在事件中注入 wall-clock 时间，避免验收不稳定。

### 3.3 隐私与路径约束

events 不得泄露真实磁盘路径（应使用虚拟路径，如 `/a.txt`、`/conversation_history/...`）。

## 4. E2E 场景（事件流必测）

### RE-01：零工具收敛的事件最小序列

给定：

- messages=[User("hello")]
- 第 1 轮返回 Assistant("world") 无 tool_calls

当：运行 Runner

则：events 形态满足：

- 包含 ModelRequestBuilt(1)
- 紧随 AssistantMessage(1)
- 紧随 RunFinished(no_tool_calls)
- 不包含 ToolCallStarted/Finished

### RE-02：单工具闭环的事件序列

给定：

- 第 1 轮输出 1 个 tool_call
- 第 2 轮输出终止 assistant

当：运行 Runner

则：

- 第 1 轮必出现 ToolCallStarted/Finished/ToolMessageAppended
- ToolMessageAppended 的 tool_call_id 与 ToolCallStarted/Finished 一致
- 第 2 轮不应出现任何 tool 相关事件

### RE-03：多工具串行的事件分组

给定：

- 第 1 轮输出 3 个 tool_calls

当：运行 Runner

则：

- events 中出现三组 tool 事件，每组内部顺序固定
- 三组之间顺序与 tool_calls 列表顺序一致

### RE-04：工具错误的事件表现必须可判定

给定：

- 某个工具返回 error

当：运行 Runner

则：

- ToolCallFinished 中明确标记 error
- ToolMessageAppended 中的内容也必须可判定为 error
- RunFinished 的统计字段（若存在）应反映错误计数变化

### RE-05：interrupt 的事件边界

给定：

- interrupt_on 命中某工具
- 第 1 轮输出该工具的 tool_call

当：运行 Runner

则：

- 产生 Interrupt 事件后立即终止本次 run（RunFinished(interrupted)）
- 不应出现 ToolCallStarted（因为工具未执行）
- tool_call_id 仍可用于后续 resume 关联

### RE-06：state update 的“发生点”必须正确

给定：

- 某工具返回 Command.update

当：运行 Runner

则：

- StateUpdated 事件必须出现在 ToolMessageAppended 之后或之前（二者择一但要固定）
- StateUpdated 必须明确包含被更新的 key（最少 key 列表）

## 5. 通过标准

- RE-01 ~ RE-06 全通过
- events.jsonl 可用于做 golden snapshot（字段稳定，不引入随机/时间噪声）

