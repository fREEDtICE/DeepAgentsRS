---
title: Core Runner E2E - 闭环与终止性
scope: core
---

## 1. 端到端效果（Runner 闭环）

给定：

- 初始 state（至少包含 messages）
- middleware 链（可为空或包含任意组合）
- backend（任意实现）
- model（ScriptedModel 或真实 provider）

Runner 必须能完成闭环：

- 构造模型请求（system + messages + tools）
- 接收 assistant message（可含 tool_calls）
- 对 tool_calls 逐个执行（串行），把工具结果回注为 ToolMessage（tool_call_id 对齐）
- 合并 Command.update 到 state
- 继续下一轮，直到收敛或达到明确终止条件

## 2. 验收前置（必须）

- 使用 ScriptedModel（确定性），除非专门做真实模型冒烟
- 固定 `recursion_limit`（例如 50）
- artifacts 输出：
  - `events.jsonl`
  - `final_state.json`
  - `backend/`（受控临时目录）

## 3. E2E 场景（必测）

### RL-01：零工具收敛（单轮）

给定：

- messages=[User("hello")]
- 第 1 轮返回 Assistant("world")，无 tool_calls

当：运行 Runner

则：

- 只发生 1 次模型调用
- final_state.messages 末尾是 Assistant("world")
- 终止原因是 no_tool_calls（或等价枚举）

### RL-02：单工具闭环（两轮）

给定：

- 工具 `echo(text)->"ECHO:"+text`
- 第 1 轮输出 tool_call：echo(text="a")
- 第 2 轮输出 Assistant("done")

当：运行 Runner

则：

- echo 被执行恰好 1 次
- tool_call_id 在 ToolMessage 中对齐
- 第 2 轮模型输入包含该 ToolMessage（ScriptedModel 断言）

### RL-03：同轮多工具串行（顺序不可变）

给定：

- 工具 t1()->"1"，t2()->"2"，t3()->"3"
- 第 1 轮 output tool_calls=[t1(id=a), t2(id=b), t3(id=c)]
- 第 2 轮 output Assistant("done")

当：运行 Runner

则：

- 执行顺序严格为 a→b→c
- ToolMessage 回注顺序严格为 a→b→c
- 任意一个工具失败时，不得静默跳过后续工具（见 RL-04/05）

### RL-04：工具失败但可恢复（不中止）

给定：

- 工具 maybe_fail(mode)：mode="fail" 返回错误；mode="ok" 返回 "OK"
- 第 1 轮调用 maybe_fail("fail")
- 第 2 轮在看到错误 ToolMessage 后调用 maybe_fail("ok")
- 第 3 轮输出 Assistant("done")

当：运行 Runner

则：

- 第 1 次工具错误被显式回注为 ToolMessage（必须可判定为 error）
- Runner 继续下一轮
- 最终收敛到 "done"

### RL-05：工具失败的“致命阈值”与终止原因

给定：

- 工具 always_fail() 永远返回错误
- ScriptedModel 每轮都调用 always_fail()
- 配置 max_consecutive_tool_errors=3（或等价机制）

当：运行 Runner

则：

- 连续错误到达阈值后终止
- 终止原因明确（fatal_tool_error / tool_error_limit 等）
- final_state 保留可诊断信息（最后一个错误 ToolMessage、轮次号）

### RL-06：recursion_limit 防死循环

给定：

- ScriptedModel 永远输出同一个 tool_call，永不终止
- recursion_limit=10

当：运行 Runner

则：

- 终止原因为 recursion_limit
- 轮次数（或模型调用次数）不会超过上限

### RL-07：Command.update 合并（patch 语义）

给定：

- 工具 set_kv(k,v) 返回 Command.update={"kv":{k:v}}
- 第 1 轮 set_kv("a",1)
- 第 2 轮 set_kv("b",2)
- 第 3 轮 Assistant("done")

当：运行 Runner

则：

- final_state.kv 同时包含 a 与 b（合并策略固定且可解释）
- events 中能看到每次 state update 的 key（至少包含 "kv"）

### RL-08：缺失 tool_call_id 的处理必须可诊断

给定：

- ScriptedModel 产出一个 tool_call 但 id 缺失/为空

当：运行 Runner

则：

- Runner 不得产生“无法对齐的 ToolMessage”
- 行为必须固定：
  - 方案 A：立即终止并报告协议错误（推荐）
  - 方案 B：将该 tool_call 视为错误并回注一条明确错误消息，但不执行工具

### RL-09：middleware 顺序对行为有决定性影响（可观测）

给定：

- MW1：在 system 末尾追加 "A"
- MW2：在 system 末尾追加 "B"
- 以顺序 [MW1, MW2] 运行一次；以 [MW2, MW1] 运行一次

当：运行 Runner

则：

- 两次模型输入 system 的末尾拼接顺序可观察（"AB" vs "BA"）
- 用于证明 middleware 是“有序链”，而非无序集合

## 4. 通过标准

- RL-01 ~ RL-09 全通过
- 任意失败都能从 events.jsonl 定位到轮次、tool_name、tool_call_id 与终止原因

