---
title: Extras 技能生态 E2E - 发现与覆盖（sources/override）
scope: extras
---

## 1. 端到端效果

技能系统必须支持从多个 source 加载技能，并对同名技能给出确定的覆盖规则：

- sources 按顺序加载
- 同名技能冲突时，后加载者覆盖先加载者（last one wins）
- 被覆盖的技能不应残留为重复工具（tool 名唯一）
- 所有加载结果可诊断（能列出最终生效的技能列表及来源）

## 2. 验收环境

- 准备两个 skills source：
  - `/skills/A/`：包含 skill `web-research`
  - `/skills/B/`：也包含 skill `web-research`（不同描述/实现）
- CLI/TUI 需提供一种方式查看“已加载技能清单”（命令或面板）

## 3. E2E 场景（必测）

### SD-01：单 source 加载

给定：

- sources=[A]

当：启动 agent

则：

- 生效技能列表包含 web-research
- 其来源显示为 A

### SD-02：多 source 加载与覆盖

给定：

- sources=[A,B]

当：启动 agent

则：

- 生效技能列表仍只有一个 web-research
- 其来源显示为 B
- tool schema/描述与 B 一致

### SD-03：覆盖后旧版本不可被调用

给定：

- sources=[A,B]
- B 版本的 web-research 行为与 A 可区分（例如返回固定标记 "B_IMPL"）

当：模型调用 web-research

则：

- 执行结果体现为 B_IMPL
- 不可能出现 A_IMPL

### SD-04：无效 source 的错误语义

给定：

- sources=[/skills/NOT_EXIST/]

当：启动 agent

则（二选一，必须固定并文档化）：

- 方案 A：启动失败并给出明确错误（推荐）
- 方案 B：启动成功但跳过该 source，并在 diagnostics 中记录告警

## 4. 通过标准

- SD-01 ~ SD-04 全通过

