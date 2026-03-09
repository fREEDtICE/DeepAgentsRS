---
title: DeepAgents Rust Core 端到端验收方案（第一期）
scope: core
generated_at: 2026-03-05
---

## 0. 文档结构（按能力拆分）

本文件是 Core 验收的总纲与入口索引。每种 Core 能力都有一份独立的端到端验收文档，覆盖：

- 端到端目标（E2E 效果是什么）
- 场景集合（Given/When/Then）
- 观测面与产物（events/state/artifacts）
- 通过标准与常见失败模式

能力文档列表（更细拆分）：

- Runner（闭环/事件）： [runner/index.md](runner/index.md)
- Backend（协议/路由/沙箱）： [backend/index.md](backend/index.md)
- Filesystem（文件/搜索/安全/执行/落盘）： [filesystem/index.md](filesystem/index.md)
- Subagents（task/隔离/回传）： [subagents/index.md](subagents/index.md)
- Summarization（event/落盘/compact/overflow）： [summarization/index.md](summarization/index.md)
- Todo（schema/merge/并行防线）： [todo/index.md](todo/index.md)
- PatchToolCalls（悬挂修复）： [patch_tool_calls/index.md](patch_tool_calls/index.md)
- HITL（interrupt/resume）： [hitl/index.md](hitl/index.md)

已有的总览文档仍保留（用于快速浏览）：

- [runner.md](runner.md)
- [backend.md](backend.md)
- [filesystem.md](filesystem.md)
- [subagents.md](subagents.md)
- [summarization.md](summarization.md)
- [todo.md](todo.md)
- [patch_tool_calls.md](patch_tool_calls.md)
- [hitl.md](hitl.md)

## 1. 目标与约束

### 1.1 验收目标（Core）

第一期 Core 的验收关注“可观察行为”一致性，而不是内部实现是否与 Python 同构。通过端到端场景验证以下能力在一个完整运行闭环中成立：

- 工具调用驱动的 agent loop：模型输出 tool_calls → 工具执行 → ToolMessage 回注 → 继续运行直至收敛
- middleware 链的拦截语义：模型调用前注入/过滤 tools 与 system 指令；工具调用后可改写结果并更新 state
- backend 协议边界：文件读写/grep/glob/执行由 backend 提供，runner 与 middleware 不直接绑定具体环境
- 安全与稳定机制：validate_path、execute gating、large tool result offload、patch 悬挂 tool_calls、todo 并行调用拒绝
- subagent 隔离：子任务上下文隔离、状态过滤、只回传最后一句
- summarization event 机制：用 `_summarization_event` 改写模型看到的 messages，并把逐出历史落盘到 `/conversation_history/{thread_id}.md`
- HITL：指定工具点可 interrupt，并可通过 resume 载荷续跑完成

### 1.2 不在第一期验收范围（Core 非目标）

- Textual/TUI 体验与渲染细节
- skills 生态（SKILL.md 的加载、脚本执行、权限控制）
- provider 特性（prompt caching、tracing/telemetry、langsmith 对接）
- 高级并发与性能指标（吞吐、延迟、内存占用）——可在第二期补充基准

## 2. 验收方法：端到端“脚本化模型” + 可选“真实模型冒烟”

### 2.1 为什么不能直接依赖真实 LLM 做自动验收

真实 LLM 的输出具有随机性与策略漂移，不适合作为 CI 的硬验收标准。Core 的端到端验收应以“确定性”优先，验证我们系统的行为边界与状态语义。

### 2.2 基本策略

端到端验收分两套：

- A. 确定性 E2E（强制，CI 运行）
  - 用一个“脚本化模型（ScriptedModel）”替代真实 provider：按预置脚本输出 assistant message/tool_calls/usage，驱动 runner 完整闭环。
  - 这种方式依然是端到端：runner + middleware + tool dispatcher + backend 全链路跑通，只是模型输出可控。
- B. 真实模型冒烟（可选，人工/夜间任务）
  - 用一个真实 provider 跑少量高层场景，验证整体可用性与 prompt/tool schema 的现实兼容性。
  - 结果不作为“严格一致性判定”，只作为回归信号。

### 2.3 端到端验收必须具备的观测面

为保证验收“可判定”，Core 需要暴露以下可观测输出（不要求具体格式，但必须可被脚本消费）：

- 事件流（至少包含）：assistant message、tool_call 开始/结束、tool result（或 ToolMessage）、state update、interrupt
- 最终 state 快照（messages/todos/_summarization_event/files 等）
- backend 侧副作用（落盘文件内容、写入路径、执行输出）

## 3. 验收环境与固定前置

### 3.1 两类 backend 组合

所有确定性 E2E 场景统一用可重复环境：

- `StateBackend`：用于验证 `files_update` 的 state patch 语义、以及“虚拟文件系统”的路径行为
- `FilesystemBackend(root_dir=tempdir)`：用于验证真实落盘内容与路径路由（在受控临时目录内）
- `CompositeBackend`：用于验证 `/conversation_history` 与 `/large_tool_results` 的路由落盘策略

### 3.2 线程标识（thread_id）

验收场景必须固定 thread_id（例如 `thread_id="e2e_thread"`），以便断言落盘路径为：

- `/conversation_history/e2e_thread.md`

并确保多轮 summarization 会追加 section 而不是覆盖。

### 3.3 Token/长度阈值

为稳定触发“截断/落盘/压缩”等逻辑，验收需要允许在测试配置中把阈值调小：

- large tool result offload 的 token 阈值
- summarization 的 trigger/keep/cutoff 规则（或用脚本化模型返回 ContextOverflow/usage 触发）

原则：阈值可配置，但行为模板与路径必须与规范一致。

## 4. 场景映射（能力 → E2E 验收文档）

Core 的 E2E 验收不追求“用例数量少”，而是追求每种能力都具备一个可复现的端到端效果定义与覆盖面。下面是映射关系：

- Runner（工具调用闭环、事件流、确定性模型）：见 [runner.md](runner.md)
- Backend（State/Filesystem/Composite、虚拟路径路由、thread_id）：见 [backend.md](backend.md)
- Filesystem（ls/read/write/edit/glob/grep/execute、validate_path、图片 read）：见 [filesystem.md](filesystem.md)
- Subagents（task 工具、隔离、只回传最后一句、CompiledSubAgent）：见 [subagents.md](subagents.md)
- Summarization（_summarization_event、offload、链式 cutoff、compact）：见 [summarization.md](summarization.md)
- Todo（write_todos 规则、并行拒绝、state 合并）：见 [todo.md](todo.md)
- PatchToolCalls（悬挂修复、恢复历史一致性）：见 [patch_tool_calls.md](patch_tool_calls.md)
- HITL（interrupt/resume，approve/reject/edit）：见 [hitl.md](hitl.md)

## 5. 通过标准（Definition of Done）

Core 端到端验收通过需满足：

- 上述各能力文档中的“必测场景”全部在确定性 E2E 模式下通过
- 每个场景都具备可复现的 artifacts（临时目录内容、最终 state、事件流快照）
- 在相同输入下多次运行结果完全一致（bitwise 或结构等价）

建议额外提供一条“真实模型冒烟”手动步骤，但不作为硬门槛：

- 真实 provider + FilesystemBackend(tempdir) 下跑一个小任务（写文件、grep、task 子代理），确认 tool schema 兼容与整体可用性。

## 6. 实现提示（为了保证验收可落地）

- ScriptedModel 必须能：
  - 按轮次返回 assistant message（含 tool_calls）
  - 可选返回 usage/token 数据（用于 summarization eligibility/trigger）
  - 可模拟 ContextOverflow 错误（用于验证 fallback summarization）
- Runner 必须输出结构化事件流，至少能让测试框架断言：
  - tool_call_id 对齐
  - interrupt 在哪个工具点触发
  - state_update 的 key 与值变化
- Backend 需要支持“虚拟路径前缀 + root_dir”映射，确保 `/conversation_history/...` 等虚拟路径最终能落在受控目录内，便于断言。
