---
title: Core 验收 - HITL（interrupt/resume 端到端）
scope: core
---

## 1. 能力定义（E2E 效果）

HITL（Human-in-the-loop）的端到端效果是：当配置 `interrupt_on` 命中某个 tool call 时，Runner 必须在执行该工具前暂停，向上层暴露一个可判定的 `Interrupt` 事件，并允许上层通过 `resume` 载荷决定：

- approve：按原参数继续执行
- reject：取消该 tool call（不产生副作用），并把“已拒绝”的结果回注为 ToolMessage
- edit：修改 tool args 后再执行

该机制必须保证：

- tool_call_id 全程对齐
- 不会重复执行（resume 只会让该 tool call 被执行或被取消一次）
- 事件流可被上层 UI/CLI 消费并驱动续跑

参考 Python 使用方式（CLI）： [non_interactive.py:L488-L564](../../../deepagents/libs/cli/deepagents_cli/non_interactive.py#L488-L564)。

## 2. 对外契约（必须明确并固定）

### 2.1 Interrupt 事件内容

Interrupt 至少包含：

- tool_name
- tool_call_id
- proposed_args（原始参数）
- 可选：policy（是否允许 edit、是否允许 reject、提示文案）

### 2.2 Resume 载荷（上层输入）

必须支持三类 resume：

- `{"type":"approve"}`
- `{"type":"reject","reason":"...可选..."}`
- `{"type":"edit","args":{...新的 args...}}`

具体字段名可调整，但必须在文档中固定，并通过 E2E 断言验证。

## 3. 验收环境

- backend=FilesystemBackend(tempdir_workspace)
- 使用 ScriptedModel 产出会触发 interrupt 的 tool_call（推荐 edit_file 或 write_file）
- Runner 支持“暂停后续跑”：第一次 run 返回 interrupted 状态与 interrupt 事件；第二次 run 输入同一 state + resume 载荷继续

## 4. E2E 场景（HITL 必测）

### H-01：approve（按原参数执行）

给定：

- interrupt_on={"edit_file": true}
- 初始文件 `/a.txt` 内容为 "1"
- 模型输出 tool_call：edit_file(file_path="/a.txt", old_string="1", new_string="2", replace_all=false)

当：

1) 运行 Runner，产生 Interrupt 并暂停
2) 注入 resume={"type":"approve"}，继续运行直至收敛

则：

- `/a.txt` 内容变为 "2"
- ToolMessage 与 tool_call_id 对齐，且可判定为“已执行并批准”
- events 中存在 Interrupt 与后续 ToolCallStarted/Finished

### H-02：reject（取消执行，不产生副作用）

给定：

- interrupt_on={"write_file": true}
- 模型输出 tool_call：write_file(file_path="/deny.txt", content="x")

当：

1) 运行 Runner 产生 Interrupt
2) resume={"type":"reject","reason":"no"} 继续

则：

- workspace 中不存在 deny.txt
- 主线程仍会得到一个与 tool_call_id 对齐的 ToolMessage（可判定为“已拒绝/已取消”）
- runner 能继续后续轮次（脚本可让模型在看到拒绝后改写到允许路径）

### H-03：edit（修改参数后执行）

给定：

- interrupt_on={"write_file": true}
- 模型输出 tool_call：write_file(file_path="/a.txt", content="1")

当：

1) Interrupt 暂停
2) resume={"type":"edit","args":{"file_path":"/b.txt","content":"2"}} 继续

则：

- workspace 中不存在 a.txt，但存在 b.txt 且内容为 "2"
- ToolMessage 对齐原 tool_call_id，并能表明参数已被修改（方式不限，但需可诊断）

### H-04：连续多次 interrupt（多工具链路）

给定：

- interrupt_on={"write_file": true, "edit_file": true}
- 模型在同一轮输出 tool_calls：[write_file(id=a,...), edit_file(id=b,...)]

当：

1) run → 在 a 处 interrupt
2) approve a → 继续 run → 在 b 处 interrupt
3) approve b → 继续 run → 收敛

则：

- 每次 interrupt 都只针对“下一个将执行的 tool_call”
- 执行顺序与 tool_calls 顺序一致
- 不会出现 “b 被跳过” 或 “a/b 被重复执行”

### H-05：resume 载荷无效时的错误语义

给定：

- 已产生 Interrupt

当：

- 输入 resume={"type":"edit","args":{缺少必需字段}}

则：

- Runner 返回明确错误（参数校验失败），并保持 state 不变（不执行工具）
- 上层可再次提交有效 resume（同一个 interrupt 不应丢失）

## 5. 通过标准

- H-01 ~ H-05 全通过
- 每个场景都能从 artifacts 中断言：
  - 文件副作用是否发生
  - ToolMessage/Interrupt 的 tool_call_id 对齐
  - 续跑不会重复执行

