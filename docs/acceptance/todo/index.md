---
title: Todo 验收索引（Core）
scope: core
---

Todo 的 E2E 验收拆分为：

- 合并/替换语义（merge=false vs merge=true）： [merge.md](merge.md)
- 并行调用防线（同轮多 write_todos 必拒绝）： [parallel_guard.md](parallel_guard.md)

工具契约（对外 schema）固定为：

- `todos: [{id, content, status, priority}]`
- `merge: bool`
- `summary?: string`（仅在发生完成态转移时允许）

