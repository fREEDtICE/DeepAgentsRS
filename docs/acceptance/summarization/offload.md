---
title: Core Summarization E2E - 历史落盘（/conversation_history）
scope: core
---

## 1. 端到端效果

历史落盘的效果是：被 summarization 逐出的消息不会丢失，而是以可追溯形式追加到：

`/conversation_history/{thread_id}.md`

并且：

- 写入是追加（append），不会覆盖旧历史
- 落盘内容不应包含摘要消息本身（避免 summary-of-summary 重复存储）
- 落盘失败不应阻断 summarization（event.file_path 可以为 null）

## 2. 验收环境

- backend=CompositeBackend
  - `/conversation_history/` → FilesystemBackend(tempdir_history)
- 固定 thread_id="e2e_thread"

## 3. E2E 场景（必测）

### SO-01：第一次落盘生成文件

给定：

- 触发一次 summarization

当：执行落盘

则：

- tempdir_history 下存在 `conversation_history/e2e_thread.md`
- 文件包含一次追加 section（标题格式可不同，但必须可判定为一次事件）
- section 内容包含被逐出的对话片段

### SO-02：多次落盘必须追加而非覆盖

给定：

- 连续触发两次 summarization/compact

当：检查落盘文件

则：

- 文件包含至少两个 section，按时间/顺序追加
- 第一段内容仍存在（未被覆盖）

### SO-03：过滤摘要消息（避免重复保存 summary_message）

给定：

- summary_message 中包含独特标记 "SUMMARY_MARK"

当：触发下一次落盘

则：

- conversation_history 文件不包含 "SUMMARY_MARK"

### SO-04：落盘路径必须使用虚拟路径引用

给定：

- event.file_path 被写入 state

当：检查 event.file_path

则：

- 必须是 `/conversation_history/e2e_thread.md`（虚拟路径）
- 不得出现 tempdir 的真实路径

### SO-05：落盘失败不阻断

给定：

- `/conversation_history/` 路由到一个只读 backend 或故意失败 backend

当：触发 summarization

则：

- `_summarization_event` 仍生成
- file_path 为 null（或缺失）
- events 中包含可诊断错误，但 runner 继续运行

## 4. 通过标准

- SO-01 ~ SO-05 全通过

