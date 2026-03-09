---
title: Runner 验收索引（Core）
scope: core
---

Runner 验收按“闭环正确性”和“可观测性”拆分为两个子文档：

- 闭环与终止性： [loop.md](loop.md)
- 事件流与可断言性： [events.md](events.md)

本组验收的目标是：不关心具体工具/后端实现细节，只验证 Runner 作为执行引擎在端到端链路中的行为边界。

