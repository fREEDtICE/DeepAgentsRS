---
title: Core Summarization E2E - 事件模型（_summarization_event）
scope: core
---

## 1. 端到端效果

Summarization 的核心不是“删除 messages”，而是用事件来改变模型看到的上下文：

- 写入私有 state key：`_summarization_event`
- 在下一轮模型调用前，把 state.messages 映射成 effective messages：
  - effective = `[summary_message] + messages[cutoff_index..]`

链式压缩必须正确：多次 summarization 时 cutoff 折算不会重复丢失或重复包含消息。

参考 Python： [summarization.py](../../../../deepagents/libs/deepagents/deepagents/middleware/summarization.py)。

## 2. 事件结构（必须可断言）

`_summarization_event` 至少包含：

- `cutoff_index: int`
- `summary_message: Message`（必须可判定为“摘要消息”）
- `file_path: string|null`（指向 `/conversation_history/{thread_id}.md`，失败可为 null）

## 3. 关键语义（必须对齐）

### 3.1 effective messages 重建

- 无 event：effective == messages
- 有 event：effective == `[summary_message] + messages[cutoff_index..]`

### 3.2 链式 cutoff 折算（最关键）

若 prior_event 已存在，则新的 cutoff 往往先基于 effective 索引产生，需要折算回 state 索引：

`state_cutoff = prior_cutoff + effective_cutoff - 1`

## 4. 验收环境

- backend=CompositeBackend
  - `/conversation_history/` → FilesystemBackend(tempdir_history)
  - default → FilesystemBackend(tempdir_workspace)
- 固定 thread_id="e2e_thread"
- 触发机制：
  - 阈值调小触发，或
  - ScriptedModel 模拟 ContextOverflow（fallback 见 overflow 文档）

## 5. E2E 场景（必测）

### SE-01：第一次触发产生 event

给定：

- 对话足够长以触发 summarization

当：触发 summarization

则：

- state._summarization_event 存在
- cutoff_index > 0 且 < len(messages)
- summary_message 可判定为摘要（例如携带固定标记或 role/metadata）

### SE-02：event 必须影响下一轮模型输入

给定：

- SE-01 完成后继续运行下一轮
- ScriptedModel 在该轮断言输入：
  - messages[0] == summary_message
  - 后续 messages 与 state.messages[cutoff_index..] 对齐

当：构造下一轮模型请求

则：断言成立

### SE-03：链式 summarization cutoff 折算正确

给定：

- 继续对话并触发第二次 summarization
- ScriptedModel 在第二次 summarization 前后记录：
  - prior_cutoff
  - effective_cutoff
  - new_state_cutoff

当：第二次 summarization 完成

则：

- new_state_cutoff == prior_cutoff + effective_cutoff - 1
- new_state_cutoff 单调递增

### SE-04：summary_message 不应在 effective 中堆叠多条

给定：

- 已经有 prior event

当：再次触发 summarization

则：

- 模型输入的 effective messages 中最多只有 1 条摘要消息作为第 0 条

### SE-05：旧 tool args 截断（降低 token）只影响模型可见，不影响副作用

给定：

- 历史中存在很大的 write_file/edit_file args（例如 content 巨长）
- 开启 truncate_args 设置

当：构造模型请求

则：

- 旧消息中的长 args 被截断为“前缀 + truncation_text”
- 最近窗口内不被截断（keep 生效）
- 文件真实内容（backend 或 state 中）不因截断而改变

## 6. 通过标准

- SE-01 ~ SE-05 全通过

