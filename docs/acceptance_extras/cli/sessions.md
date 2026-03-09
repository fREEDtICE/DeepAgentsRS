---
title: Extras CLI E2E - 会话/线程与恢复
scope: extras
---

## 1. 端到端效果

CLI 必须提供可用的会话语义，至少覆盖：

- thread_id 的选择与持久化（同一会话可恢复）
- 历史/状态的加载（从 artifacts/checkpoint 或 store）
- 多会话并存与选择（可选，但如果提供就必须可断言）

端到端的目标是：用户可以在两次 CLI 运行之间继续同一条对话线程，且不会破坏 tool_call 对齐、summarization event、todo 等状态。

## 2. 验收环境

- 使用 ScriptedModel（确定性）
- 提供一种可持久化的 checkpointer/store（例如文件/SQLite）
- CLI 提供固定接口（示例，具体可调整但需固定）：
  - `--thread-id <id>`：显式指定 thread_id
  - `--resume`：从 checkpoint 恢复
  - `--list-threads`：列出可恢复线程（可选）

## 3. E2E 场景（必测）

### CS-01：显式 thread_id 的 conversation_history 落盘路径一致

给定：

- 第一次运行指定 `--thread-id e2e_thread`
- 触发 summarization 落盘

当：检查 artifacts

则：

- 出现 `/conversation_history/e2e_thread.md`（或其映射路径）

### CS-02：同一 thread_id 的二次运行可继续对话

给定：

- Run1：写文件 `/a.txt`
- Run2：同 thread_id 读取 `/a.txt`

当：执行 Run2（从 checkpoint 恢复）

则：

- read_file 能读取到 Run1 写入内容
- messages 历史仍然对齐（无悬挂 tool_call；如有则应被 PatchToolCalls 修复）

### CS-03：不同 thread_id 状态隔离

给定：

- Run1：thread_id=t1 写入 `/a.txt`
- Run2：thread_id=t2 读取 `/a.txt`

当：执行

则：

- 若 workspace 共享磁盘，文件可能可见；但对话 state（messages/todos/_summarization_event）必须隔离
- 若 CLI 设计为“每线程独立 workspace”，则文件也应隔离（两者择一，但必须固定并文档化）

### CS-04：恢复时 PatchToolCalls 生效

给定：

- 人为构造 checkpoint：包含悬挂 tool_call

当：CLI `--resume` 继续运行

则：

- 不崩溃
- 继续运行前历史被修复（可从 events 诊断）

### CS-05：升级/版本兼容的失败语义（可选但强建议）

给定：

- 用旧版本写入的 checkpoint（schema 不一致）

当：新版本 CLI 尝试恢复

则：

- 给出明确错误与迁移建议（或自动迁移成功）
- 不应静默丢状态

## 4. 通过标准

- CS-01 ~ CS-04 必须通过
- CS-05 如果实现了恢复功能，强建议纳入门槛

