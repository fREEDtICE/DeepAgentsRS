---
title: Core 验收 - PatchToolCalls（修复悬挂 tool_calls）
scope: core
---

## 1. 能力定义（E2E 效果）

PatchToolCalls 的端到端效果是：当历史 messages 中存在“assistant 发起了 tool_call，但没有对应的 ToolMessage”时，系统在继续运行前必须修复历史一致性，避免：

- UI/日志无法对齐 tool_call 与 tool_result
- 后续轮次把悬挂 tool_call 当成“仍未执行”导致重复执行或状态错乱

参考 Python 实现： [patch_tool_calls.py](../../../deepagents/libs/deepagents/deepagents/middleware/patch_tool_calls.py)。

## 2. 对外契约（必须对齐）

- 修复发生在 run 开始阶段（在第一轮模型调用之前）
- 修复方式：为每个悬挂 tool_call 注入一条 ToolMessage（tool_call_id 对齐），内容表述为“已取消/缺失补齐/未执行”均可，但必须：
  - 可被机器识别为补齐消息（例如固定前缀或 status=error）
  - 不会触发真实工具执行

## 3. 验收环境

- 直接构造初始 state.messages（包含异常历史）
- Runner 安装 PatchToolCallsMiddleware，并在 events 中记录补丁行为（可选但推荐）

## 4. E2E 场景（PatchToolCalls 必测）

### PT-01：单个悬挂 tool_call 被补齐

给定：

- messages 包含：
  - Assistant(tool_calls=[{id:"x", name:"write_file", args:{...}}])
  - 后续没有 ToolMessage(id="x")

当：启动 Runner

则：

- messages 被修复：出现 ToolMessage(tool_call_id="x") 且可判定为“取消/补齐”
- 后续运行不会再尝试执行该 tool_call（不会写文件）

### PT-02：多个悬挂 tool_call 全部补齐

给定：

- 同一条 assistant message 中有两个 tool_calls：id="a" 与 id="b"，都缺 ToolMessage

当：启动 Runner

则：

- 插入两条 ToolMessage，且顺序与 tool_calls 顺序一致（a 在前 b 在后）

### PT-03：历史一致时不应修改 messages

给定：

- 每个 tool_call 都有对应 ToolMessage

当：启动 Runner

则：

- messages 不发生变化（可用 hash/计数断言）
- events 中不出现补丁事件（如果实现了补丁事件）

### PT-04：仅补齐“确实悬挂”的 tool_call

给定：

- 一个 assistant message 有 tool_call id="a"
- 后面已经存在 ToolMessage(id="a")，但中间穿插了其他 messages

当：启动 Runner

则：

- 不应再插入第二条 ToolMessage(id="a")

### PT-05：补齐消息必须可诊断

给定：

- PT-01 场景

当：启动 Runner

则：

- 补齐的 ToolMessage 在内容或结构上必须可诊断（例如包含 "cancelled" 或 status=error）
- 便于上层 UI/日志区分“真实执行结果”与“补丁结果”

## 5. 通过标准

- PT-01 ~ PT-05 全通过
- 补丁行为不产生任何 backend 副作用
- 任何悬挂都不会导致 runner 崩溃或重复执行

