---
title: Extras CLI 验收索引
scope: extras
---

CLI 的 E2E 验收按运行模式拆分：

- 非交互模式（脚本化 stdin/stdout）： [non_interactive.md](non_interactive.md)
- 交互入口与会话/线程管理： [sessions.md](sessions.md)
- 与 sandbox/远程执行集成（可选）： [sandbox_integration.md](sandbox_integration.md)

所有 CLI 场景必须验证：

- CLI 参数解析与默认值稳定
- 与 Core runner 的事件流对齐（至少能导出 events.jsonl）
- artifacts 输出可复现（工作区文件、conversation_history、large_tool_results）

