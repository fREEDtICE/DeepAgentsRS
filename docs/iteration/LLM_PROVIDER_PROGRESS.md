# LLM Provider / Agent Streaming 进度与后续待办

更新时间：2026-03-11

## 当前结论

Rust 侧已经完成了 `AgentProvider` 对外 streaming 的主路径改造，并引入了独立的 `LlmProvider` 层。

当前系统已经具备：

- agent-level `RunEvent` 事件流
- `ResumableRunner` 的流式执行与恢复
- `SimpleRuntime` 的 stateless streaming trait
- provider-level delta 接入与 live forwarding
- openai-compatible provider 的 non-stream / stream 双路径
- tool schema 注入到 provider-native tools payload
- provider capability 声明与 CLI/ACP 侧诊断透出

## 已完成项

### 1. Runtime / Runner Streaming

- 新增 `RunEvent` / `RunEventSink`
- `ResumableRunner` 支持：
  - `run_with_events()`
  - `resume_with_events()`
- interrupt 作为 first-class event 暴露
- `RunFinished` 已在错误/中断/完成路径统一闭环

### 2. Runtime 分层收敛

- `SimpleRuntime::run()` 已收敛为对 `ResumableRunner` 的薄包装
- `StreamingRuntime` 已调整为 stateless runtime trait：
  - `run_with_events(&self, messages, sink)`
- `SimpleRuntime` 已正式实现 `StreamingRuntime`
- `ResumableRunner` 保留 stateful inherent streaming API，不再硬套该 trait

### 3. Provider Delta Streaming

- 引入 `ProviderEvent` / `ProviderEventCollector`
- 引入 `LlmProvider`
  - `chat()`
  - `stream_chat()`
  - `capabilities()`
- `LlmProviderAdapter` 已支持：
  - streaming provider 时走 `stream_chat()`
  - non-streaming provider 时自动回退 `chat()`

### 4. Live Delta 行为修正

- `run_with_events()` 已改为 live forwarding，不再把 provider delta 缓冲到 `Vec` 后统一回放
- 事件顺序已修正为：
  - delta 在前
  - `ProviderStepReceived` 在最终稳定 step 组装完成后发出

### 5. Prompt Cache 契约

- `run()` / `resume()` 走 coarse provider path
- `run_with_events()` / `resume_with_events()` 走 live collector path
- 已明确：
  - L2 response cache 命中时只保留 coarse events
  - 不重放 `AssistantTextDelta` / `ToolCallArgsDelta` / `UsageReported`

### 6. Tool Schema

- `ToolSpec` 已新增 `input_schema`
- 内置工具已补 schema：
  - `ls`
  - `read_file`
  - `write_file`
  - `edit_file`
  - `delete_file`
  - `glob`
  - `grep`
  - `execute`
  - `task`
  - `compact_conversation`
- `skills_tools` 已复用 skill 自带的 `input_schema`
- openai-compatible provider 已使用真实 schema 构建 `tools`

### 7. OpenAI-Compatible Provider

- 已完成：
  - request payload 构建
  - non-stream chat completion
  - SSE streaming completion
  - chunk 聚合为 `LlmEvent`
  - 最终映射回 `ProviderStep`
- CLI / ACP 均已接通该 provider

### 8. Provider Capabilities

- 已新增 `LlmProviderCapabilities`
  - `supports_streaming`
  - `supports_tool_calling`
  - `reports_usage`
  - `supports_structured_output`
  - `supports_reasoning_content`
- openai-compatible provider 已声明能力
- CLI / ACP 已透出 `ProviderDiagnostics`

## 已验证项

已跑通的关键测试覆盖包括：

- `runner_events`
- `prompt_caching_provider`
- `provider_openai_compatible`
- `provider_openai_http`
- `skills_phase6`
- `deepagents-cli/e2e_runner_events`
- `deepagents-cli/e2e_openai_compatible`
- `deepagents-acp/e2e_phase3_http`

## 当前边界与已知未完成项

以下是已知仍未完成、但当前设计上已经有明确边界的部分：

### 1. Structured Output 仅有 capability，尚无请求路径

当前只完成了：

- capability 位：`supports_structured_output`

尚未完成：

- runtime / CLI / ACP 的 structured output 请求入口
- provider payload 中的 structured output 参数映射
- structured output 结果解析与错误面

### 2. Reasoning Content 仅有 capability，尚无 round-trip

当前只完成了：

- capability 位：`supports_reasoning_content`

尚未完成：

- message / state 中的 reasoning content 存储位
- provider 历史回放中的 reasoning round-trip

### 3. Provider Diagnostics 目前是 additive metadata

当前 diagnostics 主要用于：

- CLI stderr 诊断
- ACP JSON 响应诊断

尚未完成：

- diagnostics 与 event stream 的统一输出模式
- UI/TUI 的 provider capability 呈现

### 4. Provider 抽象仍未覆盖 structured output / convert_tools / native tool choice

当前 `LlmProvider` 仍是最小闭环：

- `chat`
- `stream_chat`
- `capabilities`

尚未扩展：

- `convert_tools()`
- `tool_choice`
- `structured_output()`
- provider-specific option surface

## 后续待办

建议按下面顺序推进。

### P1. Structured Output 请求路径

目标：

- 让 capability 不只是诊断字段，而是能驱动真实行为分支

任务：

- 在 CLI/ACP 增加 structured output 请求入口
- provider 不支持时，初始化阶段直接返回明确错误
- provider 支持时，扩展 request payload 和 response parse
- 补对应 contract tests

### P2. Provider-Level Tool Choice / Convert Tools

目标：

- 让 `LlmProvider` 更接近稳定的 provider adapter 层，而不只是 chat/stream 包装

任务：

- 为 `LlmProvider` 设计 `convert_tools()` 或等价工具转换接口
- 明确 tool choice 的统一抽象
- 让 openai-compatible provider 使用统一转换面而不是 request builder 里内联处理

### P3. Reasoning Content Round-Trip

目标：

- 为 thinking/reasoning models 预留历史回放能力

任务：

- 在消息模型中加入 reasoning content 保存位
- 在 provider adapter 中完成 request/response round-trip
- 明确对 event stream 的暴露方式

### P4. Capability 驱动的上层行为

目标：

- CLI / ACP / future UI 不再靠 provider 名称猜行为

任务：

- 统一 provider 初始化 bundle
- 对 streaming / structured output / usage / tool calling 全部走 capability gating
- 在 ACP SSE 或 session state 中补充 capability 元数据

### P5. Acceptance / Docs 收尾

目标：

- 让当前行为边界在验收层完全闭环

任务：

- 继续补 acceptance 文档中的 capability / structured output 条目
- 把本文件中的“已知未完成项”同步进正式 iteration plan

## 推荐下一步

最优先继续推进：

1. Structured Output 请求路径
2. Provider tool conversion / tool choice
3. Reasoning content round-trip

原因：

- runtime streaming 主体已经稳定
- cache 与 event 顺序边界已经固定
- 现在最大的能力缺口已经从“事件流”转移到“provider 语义能力”
