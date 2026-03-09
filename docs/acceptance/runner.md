---
title: Core 验收 - Runner（Agent Loop / 事件流 / 确定性 E2E）
scope: core
---

## 1. 能力定义（E2E 效果）

Runner 是 Core 的“执行引擎”。端到端效果需要满足：

- 给定初始 state、middleware 链、backend、model（脚本化或真实），Runner 能持续循环：
  - 构造模型请求（messages + system + tools）
  - 接收 assistant message（可含 tool_calls）
  - 依次执行 tool_calls（或触发 interrupt）
  - 把 tool 结果以 ToolMessage 形式回注 messages，并合并 Command.update 到 state
  - 直至收敛（assistant 无 tool_calls）或达到明确的终止条件（recursion limit/错误上限）
- Runner 产出结构化事件流（用于 CLI/UI/测试），可被确定性断言。

本验收文档只验证 Runner 自身闭环与事件语义，不验证具体工具实现细节（工具细节在各能力文档中验收）。

## 2. 验收策略（强确定性）

### 2.1 必须提供 ScriptedModel

为了让 E2E 可判定，Core 必须内置一个 ScriptedModel（只用于测试/验收），能力包括：

- 按轮次返回 assistant message（content + 可选 tool_calls）
- 可返回 usage/token（用于 summarization eligibility 等能力文档）
- 可模拟错误：例如 ContextOverflow（用于 summarization fallback）、provider error（用于 retry 策略验证）
- 可“断言输入”：即测试脚本能声明下一轮模型请求必须满足某些条件（例如 tools 是否包含 execute、messages 是否被压缩）

### 2.2 事件流（可观测输出）最低要求

Runner 必须在执行过程中发出可消费事件，最低集合：

- `ModelRequestBuilt`：包含轮次号、tools 名称集合摘要、messages 摘要
- `AssistantMessage`：完整 assistant message（含 tool_calls）
- `ToolCallStarted`：tool_name、tool_call_id、args 摘要
- `ToolCallFinished`：tool_name、tool_call_id、result 摘要或错误
- `StateUpdated`：本轮合并的 state keys 列表（不要求全量 state）
- `Interrupt`：包含 tool_call 与 proposed args（用于 HITL 文档）
- `RunFinished`：终止原因（no_tool_calls / recursion_limit / fatal_error / interrupted）

事件可以是 JSON Lines、结构体序列、或回调接口，只要 E2E 测试能稳定断言即可。

## 3. 验收前置（统一配置）

- 固定 `thread_id="e2e_thread"`（用于跨能力共享的落盘断言）
- 固定 `recursion_limit`（例如 50）以便在“无限循环脚本”场景中稳定触发
- 所有场景必须产出 artifacts 目录：
  - `events.jsonl`（或等价）
  - `final_state.json`
  - `backend/`（受控临时目录）

## 4. E2E 场景（Runner 必测）

### R-01：无工具收敛（单轮终止）

给定：

- 初始 messages = [User("hello")]
- ScriptedModel 第 1 轮返回 Assistant("world")，无 tool_calls

当：运行 Runner

则：

- 事件顺序：ModelRequestBuilt → AssistantMessage → RunFinished(no_tool_calls)
- final_state.messages 末尾为 Assistant("world")
- 总轮次数为 1

### R-02：单工具闭环（一次 tool_call）

给定：

- 注册一个 test tool：`echo_tool(text)->"ECHO:"+text`
- ScriptedModel 第 1 轮返回 tool_call：echo_tool(text="a")
- ScriptedModel 第 2 轮返回 Assistant("done")

当：运行 Runner

则：

- echo_tool 被调用恰好 1 次
- tool_call_id 在 ToolCallStarted/Finished 与 ToolMessage 中一致
- 第 2 轮模型请求 messages 必包含 ToolMessage("ECHO:a")（可由 ScriptedModel 断言输入）

### R-03：同轮多工具串行（顺序与回注）

给定：

- 两个工具：`t1()->"1"`，`t2()->"2"`
- ScriptedModel 第 1 轮输出同一条 assistant message，包含 tool_calls：[t1(id=a), t2(id=b)]
- ScriptedModel 第 2 轮输出 Assistant("done")

当：运行 Runner

则：

- 工具执行顺序与 tool_calls 列表顺序一致（先 a 后 b）
- ToolMessage 回注顺序与执行顺序一致
- 第 2 轮模型请求中，messages 中按顺序出现 ToolMessage(a) 再 ToolMessage(b)

### R-04：工具错误可恢复（非致命错误）

给定：

- 工具：`maybe_fail(mode)`，mode="fail" 返回 error，mode="ok" 返回 "OK"
- ScriptedModel 第 1 轮调用 maybe_fail("fail")
- ScriptedModel 第 2 轮在看到 error ToolMessage 后，调用 maybe_fail("ok")
- ScriptedModel 第 3 轮输出 Assistant("done")

当：运行 Runner

则：

- 第 1 次工具返回错误被封装为 ToolMessage（错误状态或错误文本需可判定）
- Runner 不中止，能进入第 2 轮并继续执行
- 最终能收敛到 Assistant("done")

### R-05：recursion_limit 触发（避免死循环）

给定：

- ScriptedModel 每轮都输出同一个 tool_call（例如 echo_tool(text="loop")），永不返回终止 assistant
- recursion_limit=10

当：运行 Runner

则：

- 事件流最终出现 RunFinished(recursion_limit)
- 工具最多被执行 10 轮（或 10 次，取决于轮次定义，但必须有硬上限）
- final_state 与 events 明确包含终止原因，便于定位

### R-06：middleware 对模型请求的影响可观测

给定：

- 安装一个 test middleware：每轮在 system message 末尾追加固定标记 "MW_MARK"
- ScriptedModel 第 1 轮断言：ModelRequestBuilt.system 必包含 "MW_MARK"
- ScriptedModel 第 1 轮返回 Assistant("ok")

当：运行 Runner

则：

- 断言成立，说明 middleware 的“模型前拦截”已生效

### R-07：Command.update 合并语义（state patch）

给定：

- 工具 `set_kv(k,v)` 返回 Command.update={"kv":{k:v}}
- ScriptedModel 第 1 轮调用 set_kv("a",1)
- ScriptedModel 第 2 轮调用 set_kv("b",2)
- ScriptedModel 第 3 轮输出 Assistant("done")

当：运行 Runner

则：

- final_state.kv == {"a":1,"b":2}（合并而非覆盖，合并策略需明确）
- 每次 StateUpdated 事件都列出被更新的 key（至少包含 "kv"）

### R-08：tool_call_id 对齐的强约束

给定：

- ScriptedModel 产出 tool_call 但 tool_call_id 为空或缺失（恶意/异常输入）

当：运行 Runner

则：

- Runner 必须以“可诊断方式”失败或将该 call 标记为 error，并在 events 中标注 tool_call_id 缺失
- 不允许出现“无法对齐的 ToolMessage”静默写入

## 5. 通过标准

- R-01 ~ R-08 全通过
- events 与 final_state 可重复（同输入多次运行一致）
- 所有失败都能从 events 定位到轮次、tool_name、tool_call_id 与错误原因

