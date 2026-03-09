---
title: Extras 技能生态 E2E - 开发者体验（脚手架/快速验证）
scope: extras
---

## 1. 端到端效果

技能生态不仅要“能跑”，还要让团队能规模化开发与维护技能。端到端验收关注：

- 脚手架：能创建一个最小技能模板（包含元数据与实现骨架）
- 校验：能在不启动完整 TUI 的情况下快速验证 skill schema 与执行
- 分发：技能目录可被 CLI 指向并加载（与 Core/Extras 集成）

## 2. 验收环境

- CLI 提供一个技能脚手架命令（示例）：
  - `deepagents skill init <dir>`
- CLI 提供一个快速校验命令（示例）：
  - `deepagents skill validate <dir>`
- 如果最终实现不使用这些命令名称，也必须提供等价入口并文档化

## 3. E2E 场景（必测）

### SDX-01：创建技能模板

当：执行 skill init

则：

- 生成的目录结构完整（元数据文件 + 实现文件）
- 默认技能能被加载（见 loading_injection 文档）

### SDX-02：validate 能发现 schema/实现错误

给定：

- 手工把必填参数从 schema 删除，或让实现返回非 JSON 结构

当：执行 skill validate

则：

- validate 返回非 0 exit code
- 输出包含可定位错误（文件路径/字段名）

### SDX-03：validate 成功时可在 CI 使用

当：执行 skill validate（正确技能）

则：

- exit code == 0
- 输出包含技能名、工具名、参数摘要

## 4. 通过标准

- SDX-01 ~ SDX-03 全通过

