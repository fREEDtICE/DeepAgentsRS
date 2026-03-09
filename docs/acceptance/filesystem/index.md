---
title: Filesystem 验收索引（Core）
scope: core
---

Filesystem 能力的 E2E 验收拆分为 5 个维度：

- 文件操作（ls/read/write/edit + 图片 read）： [file_ops.md](file_ops.md)
- 搜索能力（glob/grep + output_mode）： [search.md](search.md)
- 安全边界（validate_path 与副作用防穿越）： [security.md](security.md)
- 执行能力（execute gating 与 timeout）： [execute.md](execute.md)
- 大结果落盘（large_tool_results 引用语义）： [offload.md](offload.md)

本组验收的目标是：对齐 Python deepagents 的可观察语义边界（路径、分页、offload 模板、execute 暴露规则等），并保证在不同 backend 下依然可断言。

