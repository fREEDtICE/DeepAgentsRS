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
- 无 native tools provider 的 prompt-guided fallback
- structured output 的请求/响应初始闭环
- reasoning content 的消息层存储与 openai-compatible round-trip
- multimodal `content_blocks` 的消息层闭环与 openai-compatible request/response 适配
- mixed assistant text + tool_calls 的无损 provider step 表达
- tool schema 注入到 provider-native tools payload
- request-level `tool_choice` 抽象与 runtime/CLI/ACP 请求面接通
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

### 9. Provider Tool Conversion / Tool Choice（部分完成）

- `ProviderRequest` 已新增统一 `tool_choice`
  - `auto`
  - `none`
  - `required`
  - `named { name }`
- `LlmProvider` 已扩展 `convert_tools()`
- 已引入 typed `ToolsPayload`
  - `none`
  - `function_tools`
  - `prompt_guided`
- `ToolsPayload::PromptGuided` 已定义稳定约定：
  - 注入独立 system message
  - 使用 `<tool_call>...</tool_call>` tagged JSON contract
  - 解析回 `ToolCalls` / `AssistantMessageWithToolCalls` / 文本 step
- openai-compatible provider 已改为通过 `convert_tools()` + provider 内部 `tool_choice` 适配生成 native payload
- openai-compatible provider 已支持 native `tool_choice` 映射：
  - `auto` -> omit
  - `none` -> `"none"`
  - `required` -> `"required"`
  - `named` -> function selector object
- `named` 目标工具不存在时会返回稳定错误
- `LlmProviderAdapter` 已对 tool binding 做 capability gating：
  - provider 支持 native tool calling 时，走 native path
  - provider 不支持 native tool calling、但声明 `prompt_guided` 时，走 fallback path
  - provider 不支持两者时，拒绝 `required` / `named`
- prompt-guided path 已支持 `tool_choice` 边界：
  - `auto` -> 允许纯文本或 tagged tool payload
  - `none` -> 不注入 fallback prompt，保留纯文本路径
  - `required` -> 必须返回 tagged tool payload
  - `named` -> 目标工具不存在时初始化失败；返回其他工具名时稳定报错
- `run_with_events()` 在 prompt-guided path 下当前会退化为 coarse `chat()`：
  - 避免把 tagged JSON 原样当作 assistant delta 透出
  - 最终仍会回到稳定 `ProviderStep`
- 已明确边界：
  - CLI / ACP / runtime 只应面向统一 `tool_choice`
  - provider-native `ToolsPayload` / native payload 只留在 provider 层适配，不对 ACP 暴露
- openai-compatible 已拆分为独立子模块：
  - provider
  - transport
  - wire

### 10. Mixed Assistant Text + Tool Calls 语义补齐

- `ProviderStep` 已新增 `AssistantMessageWithToolCalls`
- openai-compatible provider 的 sync / streaming 聚合都已保留 assistant 文本与 tool calls
- runner 已按单条 assistant message 回填 `content + tool_calls`，避免适配层丢失模型原始语义
- patch middleware 已覆盖该新 step 变体，tool call id 归一化保持一致

边界说明：

- `ProviderStep` 当前只为 runtime 控制流表达必要语义
- 这次新增 mixed step 的原因是“assistant 文本 + tool calls”会直接改变 runtime 如何持久化消息并执行工具
- structured output、多模态内容块、reasoning content 不应默认继续塞进 `ProviderStep`
- 这些能力后续应优先落在 `Message` / `RunOutput` / provider normalized payload，除非它们会改变 runtime 控制流

### 11. Structured Output 请求路径（初始闭环）

- `ProviderRequest` 已新增 typed `structured_output`
- `RunOutput` 已新增 `structured_output`
- `AgentRuntimeBuilder` / `SimpleRuntime` / `ResumableRunner` 已接通 structured output 请求面
- CLI / ACP 已新增 structured output 请求入口
- `LlmProviderAdapter` 已支持 capability gating：
  - provider 不支持时返回 `provider_unsupported_structured_output`
  - CLI / ACP 初始化阶段会直接拒绝不支持的 provider
- openai-compatible provider 已映射到 native `response_format.json_schema`
- runtime 已在 final text 完成后解析 JSON，并回填到 `RunOutput.structured_output`
- 解析失败时会返回稳定错误：
  - `structured_output_invalid_response`

当前边界：

- 当前 runtime 只做 JSON parse，不做本地 schema validation
- schema 约束主要依赖 provider-native strict mode（当前 openai-compatible 已接通）
- structured output 当前只落在 `RunOutput`，没有新增专门的 event type
- provider 抽象层当前仍通过 request-level `structured_output` 传递，而不是单独的 trait method

### 12. Reasoning Content Round-Trip（初始闭环）

- `Message` 已新增 `reasoning_content`
- provider -> runtime 的非控制流 assistant 元数据已落在 typed `ProviderStepOutput`
- `run()` / `run_with_events()` / prompt cache L2 现已保留 reasoning metadata，不会在 coarse path 丢失
- openai-compatible provider 已支持：
  - request 历史中的 `reasoning_content` 回放
  - response / stream final assembly 中的 reasoning content 捕获
  - `supports_reasoning_content = true`
- event stream 当前不新增独立 reasoning delta：
  - 统一通过 `RunEvent::AssistantMessage.message.reasoning_content` 暴露
- 设计边界保持不变：
  - reasoning content 仍位于消息 / 历史回放层
  - 不新增 `ProviderStep` 变体

### 13. Multimodal Message Surface（初始闭环）

- `Message.content_blocks` 已成为统一的多模态消息承载位：
  - tool result
  - assistant metadata
  - 历史回放
- `ContentBlock` 已补稳定 helper：
  - `image_base64(...)`
  - `as_image_base64()`
  - fallback text 生成
- provider -> runtime 的 assistant multimodal 内容继续走 `ProviderStepOutput.assistant_metadata.content_blocks`
  - 不新增 `ProviderStep` 变体
  - 不把图片语义塞进 runtime 控制流
- openai-compatible provider 已支持：
  - `user` role 的 image block -> native `content[]` parts 编码
  - assistant multimodal response / stream final assembly -> `content_blocks` 解析回放
  - image-only assistant response 的稳定文本降级
- runtime / RunEvent / CLI / ACP 当前策略：
  - 继续透传 `Message.content_blocks`
  - 非多模态文本面通过稳定 fallback `content` 兜底，不新增独立 preview event
- 当前 provider 边界已显式固定：
  - 对 OpenAI Chat Completions 不支持的 role/content 组合，不发送非法 image parts
  - 统一退化为稳定文本 `content`

## 已验证项

已跑通的关键测试覆盖包括：

- `runner_events`
- `provider_prompt_guided`
- `prompt_caching_provider`
- `provider_openai_compatible`
- `provider_openai_http`
- `skills_phase6`
- `deepagents-cli/e2e_runner_events`
- `deepagents-cli/e2e_openai_compatible`
- `deepagents-acp/e2e_phase3_http`

## 当前边界与已知未完成项

以下是已知仍未完成、但当前设计上已经有明确边界的部分：

### 1. Structured Output 已有初始闭环，但仍是 V1

当前只完成了：

- capability 位：`supports_structured_output`
- runtime / CLI / ACP 的 structured output 请求入口
- openai-compatible provider 的 `response_format.json_schema` 映射
- `RunOutput.structured_output` 结果回填
- provider 不支持时的 capability gating / 初始化报错

尚未完成：

- 本地 schema validation（当前只做 JSON parse）
- 除 openai-compatible 外的更多 provider 映射
- structured output 与 event stream / UI 呈现的统一策略

### 2. Reasoning Content 已有消息层 round-trip，但仍是 V1

当前已完成：

- capability 位：`supports_reasoning_content`
- `Message.reasoning_content` 存储位
- provider 历史回放中的 reasoning round-trip
- `RunEvent::AssistantMessage` 上的 reasoning 暴露

尚未完成：

- 除 openai-compatible 外的更多 provider 覆盖
- reasoning content 在 CLI / ACP / UI 侧的专门呈现策略

### 3. Multimodal Message Surface 已有消息层闭环，但仍是 V1

当前已完成：

- `Message.content_blocks` 的 assistant/tool/history 透传
- openai-compatible provider 的：
  - `user` role multimodal request 编码
  - assistant multimodal response / stream final round-trip
  - image-only assistant 内容的稳定文本降级
- runtime replay test / provider serialization test / SSE final assembly test

尚未完成：

- 除 openai-compatible 外的更多 provider 映射
- 更丰富的 block 类型（当前主要覆盖 image base64）
- UI 侧独立 preview / export 策略

### 4. Provider Diagnostics 目前是 additive metadata

当前 diagnostics 主要用于：

- CLI stderr 诊断
- ACP JSON 响应诊断
- ACP SSE `provider_info` 事件

当前已补齐：

- `surface_capabilities` 作为归一化上层能力视图
- CLI / ACP 顶层基于 capability 做 structured output / tool_choice 预检
- ACP session state / JSON response / SSE 的 capability 输出

### 5. Provider 抽象尚未覆盖 structured output / provider-specific option surface

当前 `LlmProvider` 已具备：

- `chat`
- `stream_chat`
- `capabilities`
- `convert_tools()`
- request-level `tool_choice`（经 `ProviderRequest` 传递）

尚未扩展：

- `structured_output()`
- provider-specific option surface

## 后续待办

建议按下面顺序推进。

P1 `Prompt-guided fallback` 已完成。
P2 `Structured Output 请求路径` 已完成第一版，当前优先进入后续语义补齐。
P3 `Reasoning Content Round-Trip` 已完成第一版。
P4 `Multimodal Message Surface` 已完成第一版。

### P5. Capability 驱动的上层行为

已完成第一版。

目标：

- CLI / ACP / future UI 不再靠 provider 名称猜行为

已完成：

- 统一 provider 初始化 bundle
- 增加归一化 `surface_capabilities`，覆盖 streaming / usage / tool_choice / structured_output
- CLI / ACP 对 structured output / tool_choice 走 capability gating
- ACP session state / JSON response / SSE 均带 provider capability 元数据

### P6. Acceptance / Docs 收尾

目标：

- 让当前行为边界在验收层完全闭环

任务：

- 继续补 acceptance 文档中的 capability / structured output 条目
- 把本文件中的“已知未完成项”同步进正式 iteration plan

## 推荐下一步

最优先继续推进：

1. Acceptance / Docs 收尾
2. Structured output 的本地 schema validation / 多 provider 覆盖
3. reasoning content / multimodal 的多 provider 覆盖与 UI 呈现策略

原因：

- runtime streaming 主体已经稳定
- cache 与 event 顺序边界已经固定
- mixed step 与 `tool_choice` 主路径已经接通
- capability 驱动的上层行为已经有稳定一版，剩余缺口更偏验收闭环与多 provider 扩展
