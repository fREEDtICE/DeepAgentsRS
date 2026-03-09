---
title: Extras Tracing E2E - Trace 结构与关联
scope: extras
---

## 1. 端到端效果

一次完整运行必须产生结构化 trace，并满足可关联、可诊断：

- trace_id 唯一标识一次 run（或一次“会话内任务”）
- spans 覆盖关键阶段：
  - runner loop
  - model request/response
  - tool execution（每个 tool_call 一个 span）
  - subagent execution（每次 task 一个 span）
  - summarization/offload（如果触发）
- 关联字段齐全：
  - thread_id
  - tool_call_id
  - subagent_type（若有）
  - backend 类型（state/filesystem/composite/sandbox）

## 2. 统一字段规范（建议但必须固定）

为便于跨团队消费，建议统一 attribute 命名（可调整，但必须固定并文档化）：

- `deepagents.thread_id`
- `deepagents.run_id`
- `deepagents.tool.name`
- `deepagents.tool.call_id`
- `deepagents.subagent.type`
- `deepagents.backend.kind`
- `deepagents.result.status`（ok/error/interrupted）

## 3. 验收环境

- 使用 ScriptedModel（确定性）
- 至少包含一次：
  - 普通模型回复
  - 一个 tool_call
  - 一个 subagent task（可选但建议）

## 4. E2E 场景（必测）

### TS-01：最小 trace 存在且可解析

给定：

- 运行一次最小对话（无工具）

当：导出 trace（见 export 文档）

则：

- 至少存在 1 条 trace
- trace 可被解析为 spans 树
- root span 包含 thread_id/run_id

### TS-02：tool_call 必须生成 span 且关联 tool_call_id

给定：

- 触发 write_file tool_call_id=a

当：运行

则：

- 存在 span 名称包含 tool/write_file（或等价）
- span attributes 包含 deepagents.tool.call_id="a"
- span status=ok 或 error（取决于结果）

### TS-03：subagent task 必须生成 span 且与主 trace 关联

给定：

- 触发 task subagent_type="general-purpose"

当：运行

则：

- 存在 subagent span
- subagent span 与主 trace 在同一 trace_id（或通过 parent/links 关联，二者择一但必须固定）

### TS-04：interrupt 必须在 trace 中可见

给定：

- 触发 HITL interrupt

当：运行到暂停

则：

- trace 中存在标记 interrupted 的 span 或 event
- 能定位到被暂停的 tool_call_id

### TS-05：错误路径可诊断

给定：

- 某工具返回错误

当：运行

则：

- tool span status=error
- span attributes 不包含敏感内容（详见 redaction 文档）

## 5. 通过标准

- TS-01 ~ TS-05 全通过

