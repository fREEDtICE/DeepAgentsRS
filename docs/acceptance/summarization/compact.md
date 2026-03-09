---
title: Core Summarization E2E - 手动 compact_conversation
scope: core
---

## 1. 端到端效果

`compact_conversation` 是显式工具：让模型或用户在需要时主动压缩上下文。端到端效果必须满足：

- 工具无参数
- 未达到 eligibility gate 时，返回明确提示且不改变 state
- 达到 gate 时：
  - 更新 `_summarization_event`
  - 追加落盘到 `/conversation_history/{thread_id}.md`
  - 回注一条 ToolMessage 表示 compact 完成

## 2. 验收环境

- backend=CompositeBackend（conversation_history 路由到 tempdir_history）
- 固定 thread_id="e2e_thread"
- eligibility 阈值允许配置（为了稳定触发）

## 3. E2E 场景（必测）

### SC-01：不满足 gate 时不执行

给定：

- 对话很短

当：compact_conversation()

则：

- ToolMessage 提示“无需 compact/不满足条件”
- `_summarization_event` 不变化
- 不写入 conversation_history

### SC-02：满足 gate 时更新 event 并落盘

给定：

- 对话达到 gate（可通过阈值调小）

当：compact_conversation()

则：

- `_summarization_event` 更新
- conversation_history 文件追加一段新内容
- ToolMessage 表示 compact 完成

### SC-03：compact 与自动 summarization 共用 event 语义

给定：

- 先自动 summarization 生成 event
- 再调用 compact_conversation

当：执行 compact

则：

- cutoff 折算正确（不回退）
- effective messages 仍只有一条 summary_message 作为第 0 条

### SC-04：落盘失败仍视为 compact 成功（但 file_path 为空）

给定：

- conversation_history 写入失败

当：compact_conversation()

则：

- event 仍更新
- file_path 为 null
- ToolMessage 仍表示 compact 已完成（同时 events 含可诊断告警）

## 4. 通过标准

- SC-01 ~ SC-04 全通过

