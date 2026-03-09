---
title: Extras TUI E2E - 对话渲染与流式输出
scope: extras
---

## 1. 端到端效果

TUI 的最核心效果：用户能在终端中清晰看到“对话流 + 工具执行流”，并且 UI 不丢事件、不乱序、不重复。

具体表现：

- 用户输入后立即出现“正在生成/正在调用工具”的状态反馈
- assistant message 以流式方式追加渲染（chunk 合并策略固定）
- tool 调用以独立区域/卡片展示（哪怕是最简文本块）
- 对同一条 assistant message 的 tool_calls，显示顺序与执行顺序一致

## 2. 验收方法（TUI E2E 的可判定性）

推荐同时支持两种断言方式（至少一种必须实现）：

- 屏幕快照：在关键时间点 dump 当前屏幕文本（golden snapshot）
- 组件树快照：dump UI 树结构（节点类型、关键字段），避免受终端宽度影响

验收需在固定终端尺寸下运行（例如 120x40）。

## 3. E2E 场景（必测）

### TR-01：最小对话渲染

给定：

- ScriptedModel：User("hi") → Assistant("hello")

当：在 TUI 中发送消息

则：

- 屏幕出现用户消息与 assistant 回复
- assistant 回复只出现一次

### TR-02：流式输出 chunk 合并

给定：

- ScriptedModel 以多个增量 chunk 输出 "hel" + "lo"

当：运行

则：

- UI 最终显示为 "hello"
- 不出现重复拼接（例如 "helhello"）

### TR-03：工具调用状态渲染

给定：

- assistant 触发 write_file 工具

当：运行

则：

- UI 先显示“工具执行中”状态
- 工具完成后显示“工具结果”

### TR-04：多工具调用的顺序一致性

给定：

- 同一轮 tool_calls=[t1,t2,t3]

当：运行

则：

- UI 按 t1→t2→t3 展示
- 每个工具块都能关联 tool_call_id（至少在内部状态中可断言）

### TR-05：错误工具结果渲染（可诊断）

给定：

- 某工具返回错误

当：运行

则：

- UI 清晰展示错误（颜色/前缀不限，但文本必须可判定）
- 不崩溃，仍可继续输入下一条消息

## 4. 通过标准

- TR-01 ~ TR-05 全通过

