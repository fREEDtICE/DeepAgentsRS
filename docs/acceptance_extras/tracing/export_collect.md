---
title: Extras Tracing E2E - 导出与采集（OTLP/文件/控制台）
scope: extras
---

## 1. 端到端效果

Tracing 不仅要“生成”，还要能在真实环境被采集与分析。端到端要求：

- 至少支持一种导出方式（必须）：
  - 文件导出（JSON/NDJSON）
  - 或 OTLP 导出到本地 collector（CI 可启动）
- 导出失败必须可诊断且不影响主任务完成（不应让 agent 直接失败）
- 支持在 CLI/TUI 中通过开关启用/禁用 tracing

## 2. 验收环境

- Case A：文件导出（推荐作为 CI 强制路径）
  - `TRACING_EXPORT=file://<artifacts>/traces.ndjson`
- Case B：OTLP 导出（可选，但如果宣称支持则必须验收）
  - 启动本地 mock collector，监听 `localhost:4317`

## 3. E2E 场景（必测）

### TE-01：文件导出成功

给定：

- 开启 tracing，export=file

当：运行一次包含 tool_call 的任务

则：

- artifacts 下生成 traces.ndjson
- 文件非空，且每行可解析为 span/trace 记录

### TE-02：文件导出关闭时不产生 trace 文件

给定：

- tracing=off

当：运行任务

则：

- 不生成 traces 文件（或生成空文件，二者择一但必须固定）

### TE-03：导出失败不阻断任务

给定：

- export 目标不可写（只读目录）

当：运行任务

则：

- 任务仍成功完成（exit code==0，或按 CLI 设计）
- stderr/diagnostics 提示 tracing 导出失败

### TE-04：OTLP 导出成功（若支持）

给定：

- mock collector 在 4317 监听

当：运行任务

则：

- collector 收到 spans（可按数量下限断言）
- spans 可解析并包含 thread_id/run_id

### TE-05：采样策略（可选但建议）

给定：

- sampling=0（全不采样）或 sampling=1%（按实现支持）

当：运行多次任务

则：

- 导出 spans 数量符合预期范围（统计性断言即可）

## 4. 通过标准

- TE-01/02/03 必须通过
- TE-04 若宣称支持 OTLP 则必须通过

