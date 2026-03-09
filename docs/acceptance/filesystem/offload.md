---
title: Core Filesystem E2E - 大结果落盘（/large_tool_results）
scope: core
---

## 1. 端到端效果

大结果落盘的目标是避免上下文爆炸，同时保留“可追溯的完整结果”：

- 当某个工具返回的文本结果超过阈值，系统必须把完整结果写到：
  - `/large_tool_results/{sanitized_tool_call_id}`
- 返回给模型/用户的 ToolMessage 必须被替换为：
  - 引用文件路径（虚拟路径）
  - 头尾预览（head/tail）
  - 引导用户用 read_file(offset/limit) 分页读取

参考 Python： [filesystem.py](../../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py)。

## 2. 关键约束（对齐 Python 的精神）

- offload 必须发生在 tool 执行后、回注 messages 前（这样模型不会看到巨量文本）
- offload 的写入必须通过 backend（以便 CompositeBackend 路由隔离）
- 下列工具默认不参与 offload（Python 侧排除）：ls/glob/grep/read_file/edit_file/write_file
  - Rust 可选择完全对齐或做更合理拆分，但必须在验收中固定“哪些工具会被 offload”

## 3. 验收环境

- backend=CompositeBackend
  - default=FilesystemBackend(tempdir_workspace)
  - `/large_tool_results/` → FilesystemBackend(tempdir_large)
- offload 阈值调小（例如 50 tokens）以稳定触发
- 准备一个会产生超大输出的工具：
  - 推荐：提供一个 test tool `emit_big(n)->"x"*n`（仅用于验收）

## 4. E2E 场景（必测）

### FO-01：触发 offload 并写入完整内容

给定：

- tool_call：emit_big(n=10000)，返回超大文本

当：执行工具

则：

- tempdir_large 下出现对应文件（名称可直接用 tool_call_id 或其 sanitize 结果）
- 文件内容长度接近 10000（证明不是 head/tail）
- ToolMessage 内容不包含完整文本，只包含引用模板

### FO-02：ToolMessage 引用可回读

给定：

- FO-01 已完成

当：read_file("/large_tool_results/{id}", offset=0, limit=5)

则：

- 能读到落盘内容的前几行/片段

### FO-03：offload 不污染 workspace

给定：

- CompositeBackend 路由如上

当：触发 offload

则：

- tempdir_workspace 下不出现 large_tool_results 目录

### FO-04：offload 的排除列表行为固定

给定：

- 尝试让 read_file 返回很长内容（或 grep 返回很长内容）

当：执行

则：

- 系统要么始终不对 read_file/grep 做 offload（对齐 Python 排除），要么以明确规则做 offload
- 行为必须固定，并在本文件明确

### FO-05：sanitize_tool_call_id 的可预测性

给定：

- tool_call_id 包含特殊字符（例如 "a:b/c")

当：触发 offload

则：

- 写入路径是合法虚拟路径
- 不出现路径注入（不会变成目录穿越）

## 5. 通过标准

- FO-01 ~ FO-05 全通过
- artifacts 中包含：
  - tempdir_large 的目录快照
  - events.jsonl（证明模型未收到完整大文本）

