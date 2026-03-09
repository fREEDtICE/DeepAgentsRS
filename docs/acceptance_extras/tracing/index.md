---
title: Extras Tracing/Telemetry 验收索引
scope: extras
---

Tracing/Telemetry 的 E2E 验收按三层拆分：

- Trace 结构与关联（span/trace_id/tool_call_id/thread_id）： [trace_structure.md](trace_structure.md)
- 导出与采集（OTLP/文件/控制台）： [export_collect.md](export_collect.md)
- 脱敏与合规（PII/Secrets/文件内容）： [redaction.md](redaction.md)

目标：任何一次端到端运行都能生成可诊断的 trace，且不会泄露敏感信息。

