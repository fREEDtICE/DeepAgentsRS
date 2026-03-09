---
title: Core HITL E2E - interrupt 时序与不重复执行
scope: core
---

## 1. 端到端效果

HITL 的核心是“在工具执行前暂停”，并保证续跑后：

- 要么执行一次（approve/edit）
- 要么取消一次（reject）
- 不会重复执行

## 2. 验收环境

- backend=FilesystemBackend(tempdir_workspace)
- interrupt_on={"write_file":true,"edit_file":true}
- ScriptedModel 产出同轮多 tool_calls 的复杂场景

## 3. E2E 场景（必测）

### HF-01：interrupt 必须发生在 ToolCallStarted 之前

给定：

- 第 1 轮输出 tool_call：write_file(id=a,...)

当：运行 Runner

则：

- events 中出现 Interrupt(tool_call_id=a)
- 不应出现 ToolCallStarted(a) 或 ToolCallFinished(a)

### HF-02：approve 后只执行一次

给定：

- HF-01 后 resume approve

当：续跑

则：

- ToolCallStarted/Finished 各出现一次
- workspace 文件只写入一次（不应出现重复追加）

### HF-03：reject 后不产生副作用，但仍回注 ToolMessage

给定：

- interrupt_on={"write_file":true}
- 第 1 轮输出 write_file(id=a,file_path="/deny.txt",content="x")

当：

- run → interrupt
- resume reject

则：

- deny.txt 不存在
- 主线程仍获得 ToolMessage(tool_call_id=a) 表示拒绝

### HF-04：edit 后执行修改后的 args，且不执行原 args

给定：

- 第 1 轮输出 write_file(id=a,file_path="/a.txt",content="1")

当：

- run → interrupt
- resume edit(args={file_path:"/b.txt",content:"2"})

则：

- a.txt 不存在
- b.txt 存在且内容为 "2"

### HF-05：同轮多工具的多次 interrupt 次序固定

给定：

- 第 1 轮输出 tool_calls：[write_file(id=a,...), edit_file(id=b,...)]

当：

- 第一次 run

则：

- 只在 a 处 interrupt（先暂停第一个即将执行的 tool_call）
- b 不应先触发 interrupt

当：

- approve a 后续跑

则：

- 在 b 处触发下一次 interrupt

## 4. 通过标准

- HF-01 ~ HF-05 全通过

