---
title: Extras 技能生态 验收索引
scope: extras
---

技能生态的 E2E 验收按生命周期拆分：

- 技能发现与覆盖（多 source、last one wins）： [discovery_override.md](discovery_override.md)
- 技能加载与注入（system prompt/tool schema）： [loading_injection.md](loading_injection.md)
- 技能执行与运行时隔离（权限、沙箱、资源）： [execution_isolation.md](execution_isolation.md)
- 技能开发者体验（脚手架/快速验证）： [devx.md](devx.md)

技能的端到端目标：用户只通过配置 package skill sources（`SKILL.md` + 可选
`tools.json`）即可获得新增工具能力，且能力可控（权限/隔离），并能在 CLI/TUI 中稳定
呈现与调试。
