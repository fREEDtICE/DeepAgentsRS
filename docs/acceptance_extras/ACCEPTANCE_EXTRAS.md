---
title: DeepAgents Rust Extras 端到端验收方案（第二期）
scope: extras
generated_at: 2026-03-05
---

## 0. 范围说明

本验收方案覆盖 Extras：

- CLI（非交互/脚本化运行、与 Core runner 的集成）
- TUI（交互式界面：消息流、工具卡片、审批、diff、历史与会话）
- 技能生态（技能发现/加载/覆盖、tool schema 注入、权限与隔离）
- Provider 特性（prompt caching）
- Tracing/Telemetry（结构化 trace/span、日志、导出、脱敏）

约束：这里仍然是端到端验收（E2E），强调“效果可复现、可判定、可诊断”，不把单元测试当作验收主渠道。

## 1. 验收方法总原则

- A. 确定性 E2E（CI 强制）
  - 用 ScriptedModel 驱动行为确定；对 UI 用脚本输入与快照断言
  - 对网络/导出用本地 mock/collector（不依赖公网）
- B. 真实环境冒烟（可选）
  - 用真实 provider、真实终端交互跑少量关键路径，作为回归信号

所有 E2E 场景必须产出 artifacts（最少）：

- `events.jsonl`：runner 事件流（或等价）
- `ui.snapshots/`：TUI 截图/快照（文本屏幕快照）
- `files/`：工作区/历史/large results 的目录快照
- `traces/`：trace 导出（OTLP/JSON）与脱敏校验报告

## 2. 文档索引（按能力拆分）

- CLI： [acceptance_extras/cli/index.md](cli/index.md)
- TUI： [acceptance_extras/tui/index.md](tui/index.md)
- 技能生态： [acceptance_extras/skills/index.md](skills/index.md)
- Provider（Prompt Caching 等）： [acceptance_extras/provider/index.md](provider/index.md)
- Tracing/Telemetry： [acceptance_extras/tracing/index.md](tracing/index.md)
