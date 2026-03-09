---
title: Summarization 验收索引（Core）
scope: core
---

Summarization 的验收按机制拆分：

- 事件模型（_summarization_event / effective messages / 链式 cutoff）： [event.md](event.md)
- 历史落盘（/conversation_history 追加与过滤）： [offload.md](offload.md)
- 手动 compact_conversation： [compact.md](compact.md)
- 溢出回退（ContextOverflow fallback）： [overflow.md](overflow.md)

