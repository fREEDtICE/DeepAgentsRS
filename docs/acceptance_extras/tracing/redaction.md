---
title: Extras Tracing E2E - 脱敏与合规（PII/Secrets/文件内容）
scope: extras
---

## 1. 端到端效果

Tracing/日志是最容易泄露敏感信息的通道。端到端必须保证：

- traces/logs/events 中不出现 secrets（API key、token、password 等）
- 不记录原始文件内容（尤其是 read_file/write_file 的 content、大工具结果）
- 对必须记录的字段，使用 hash/长度/摘要代替原文
- 脱敏策略可配置且默认安全（secure by default）

## 2. 敏感信息集合（用于验收注入）

在验收中注入以下特征串，确保不会泄露：

- `SECRET_TOKEN_ABC123`
- `PASSWORD=supersecret`
- `AWS_SECRET_ACCESS_KEY=...`
- `BEGIN_PRIVATE_KEY...`

并在 workspace 文件中写入包含这些特征串的内容，验证 trace/log 不会把其原文带出。

## 3. 验收环境

- 开启 tracing 文件导出（traces.ndjson）
- 同时开启 events.jsonl 导出（runner 事件流）
- 运行包含以下行为的任务：
  - write_file 写入包含敏感串的文件
  - read_file 读取该文件
  - 一个工具返回错误（确保 error path 也不泄露）

## 4. E2E 场景（必测）

### TRD-01：traces 中不得出现敏感串

当：运行任务并导出 traces.ndjson

则：

- traces.ndjson 不包含任意敏感特征串
- 若出现即失败

### TRD-02：events 中不得出现敏感串

当：导出 events.jsonl

则：

- events.jsonl 不包含任意敏感特征串

### TRD-03：工具参数与结果脱敏

给定：

- write_file(content 含敏感串)
- read_file 返回内容含敏感串

当：检查 trace/events 的 tool args/result 字段

则：

- 不出现原文 content
- 允许出现：
  - `content_len`
  - `content_hash`
  - `preview_redacted`

### TRD-04：错误路径不泄露

给定：

- 构造一个错误，错误信息中可能包含用户输入（含敏感串）

当：检查 trace/events

则：

- 错误消息被脱敏或截断为安全摘要

### TRD-05：可配置的脱敏策略

给定：

- redaction=on（默认）
- redaction=off（仅用于本地调试，必须显式打开）

当：分别运行

则：

- on：严格不泄露（必过）
- off：可允许泄露，但必须在输出中明确标记“脱敏关闭”，避免误用（并且 CI 不允许 off）

## 5. 通过标准

- TRD-01 ~ TRD-04 必须通过
- TRD-05 若提供开关则必须通过

