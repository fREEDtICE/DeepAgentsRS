# DeepAgentsRS Provider 能力对齐讨论纪要（与 Python deepagents / zeroclaw 对照）

本文档整理并固化一次关于“DeepAgentsRS 的 Provider 能力是否需要对齐 zeroclaw 的 Provider traits，以及 Python deepagents 如何调用/stream LLM”的深入讨论，覆盖背景、关键结论、关键细节、风险点与推荐实现路径。

## 背景与问题定义

仓库内存在三套相关实现：

- DeepAgentsRS（Rust）：`/DeepAgentsRS/crates/deepagents`，当前对外暴露的是“agent loop 语义”的 Provider 协议（一步一步推进），见 [protocol.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/provider/protocol.rs)。
- Python deepagents：`/deepagents/libs/deepagents`，依赖 LangChain/LangGraph；deepagents 自己不实现 LLM 的 HTTP provider，而是以 `BaseChatModel` 为抽象把模型能力注入 agent 图构建。
- zeroclaw（Rust）：`/zeroclaw/src/providers/traits.rs`，提供更接近“LLM 客户端/能力适配层”的 Provider 抽象（capabilities、tools payload 转换、streaming 事件等），见 [traits.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/zeroclaw/src/providers/traits.rs)。

核心问题是：DeepAgentsRS 的 provider 协议要不要对齐 zeroclaw 的 Provider traits？如果要对齐，应该在什么层对齐，如何实现，风险是什么？

## 关键结论（摘要）

1. DeepAgentsRS 当前的 `Provider` 更像“Agent Provider”（决策一步返回什么动作），不是“LLM Provider”（负责发 HTTP、处理 capabilities、tool schema 转换、streaming 协议解析）。
2. zeroclaw 的 Provider traits 是“LLM Provider（通讯与能力适配层）”抽象，覆盖 capabilities、tools payload 转换、prompt-guided fallback、streaming event、usage、reasoning_content round-trip 等。
3. Python deepagents 的“LLM Provider”不是 deepagents 自己的一层：deepagents 通过 LangChain 的 `BaseChatModel` 抽象把模型能力注入图，LLM 调用与 streaming 由上游库完成；deepagents 只通过 middleware 在“模型调用前/后”对请求/状态做治理。
4. 若 DeepAgentsRS 想对齐 Python deepagents 的 streaming 体验，重点不是把 `Provider` trait 塞满 capabilities/streaming 方法，而是明确是否要引入一个“事件流层（StreamEvent）”的 runtime API；Python 的 streaming 是 agent-level event stream，而不是单纯的 LLM token stream。
5. 推荐路径：保持 DeepAgentsRS 的 step-based Agent Provider（对 HITL/工具回填/中间件链更稳定），并新增一个独立的 “LLM 客户端层 trait”（可参考 zeroclaw），再用适配器把 chat/stream 响应映射成 step 或事件流，避免职责混淆。

## 术语与层次划分

为避免“Provider”概念混淆，建议在文档/代码里明确区分：

- Agent Provider（控制流层）：输出“下一步做什么”的指令（说一句、调用工具、调用 skill、结束、报错）。对应 DeepAgentsRS 当前的 [ProviderStep](file:///Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/provider/protocol.rs#L24-L46)。
- LLM Provider（通讯/适配层）：负责与具体 LLM API 交互，声明能力（native tool calling、vision、streaming tool events 等），将统一的 tool schema 转换成各厂商格式，并提供 streaming 事件或 chunk。对应 zeroclaw 的 [Provider](file:///Users/bytedance/Documents/Dev/deepagents-rs/zeroclaw/src/providers/traits.rs#L281-L506)。

## DeepAgentsRS 当前 Provider 与运行循环语义

### ProviderStep：一步是什么

DeepAgentsRS 的 provider 每次被调用，返回一个 `ProviderStep`，表示本轮 agent loop 的动作：

- `AssistantMessage { text }`：追加一条 assistant 消息，继续下一轮。
- `FinalText { text }`：终止（Completed）。
- `ToolCalls { calls }`：请求执行一组工具调用，本轮不终止；runtime 执行工具并回填 tool messages，再进入下一轮。
- `SkillCall { name, input, call_id }`：请求调用 skill；runtime 先把 skill 展开成 tool_calls，再执行。
- `Error { error }`：结构化失败，runtime 直接终止（Error）。

定义见 [protocol.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/provider/protocol.rs#L24-L46)。

### runtime 如何消费 ProviderStep

运行循环的核心在 [simple.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/runtime/simple.rs)（以及可恢复版 [resumable_runner.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/runtime/resumable_runner.rs)）：

- 形成 `ProviderRequest { messages, tool_specs, skills, state, last_tool_results }` 后调用 provider.step。
- 执行 `patch_provider_step` 中间件做 tool_calls 修补与归一化（典型：补 call_id、修 dangling tool calls），见 [patch_tool_calls.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/runtime/patch_tool_calls.rs#L131-L163)。
- 若收到 `ToolCalls` 或 `SkillCall`，进入工具执行阶段：对入参进行归一化/校验，执行工具，生成 tool_result 并以 `role=tool` 的 message 形式写回 messages。

工具调用入参归一化的关键函数是 `normalize_tool_call_for_execution`，见 [tool_compat.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/runtime/tool_compat.rs#L160-L214)：

- tool_name 为空 → `"unknown"`
- call_id 缺失/空 → 自动生成 `"call-<n>"`
- arguments 必须为 JSON object（允许 null→{}；允许 string 且可 parse 为 object）

注意语义差异：工具执行失败（或 arguments 非法）不会立刻终止 run，而是回填 `ToolMessage(status=error)` 让 provider 下一轮自行收敛；skill 找不到等错误可能被 runtime 直接终止。

### Provider 调用的包装：timeout + prompt cache

provider.step 的调用通常经过 `step_with_prompt_cache`：

- 超时：超时映射为 `provider_timeout`
- provider.step 返回 `Err(anyhow)`：映射为 `provider_error`
- `ProviderStep::Error`：映射为 `provider_step_error`

代码见 [prompt_cache_runtime.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/runtime/prompt_cache_runtime.rs)。

## zeroclaw Provider traits 提供了哪些能力

zeroclaw 的 Provider traits 明确处于 LLM 客户端层：

- `capabilities()`：声明 `native_tool_calling`、`vision` 等能力，见 [traits.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/zeroclaw/src/providers/traits.rs#L245-L290)。
- `convert_tools()`：把统一 `ToolSpec` 转成 Gemini/Anthropic/OpenAI 等原生格式，或 fallback 为 PromptGuided 文本注入，见 [traits.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/zeroclaw/src/providers/traits.rs#L262-L303)。
- `chat()`：当 provider 不支持 native tools 时，默认实现会把 tool instructions 注入 system prompt 再调用 `chat_with_history()`，见 [traits.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/zeroclaw/src/providers/traits.rs#L348-L403)。
- streaming：`stream_chat(...) -> StreamEvent` 支持 TextDelta/ToolCall/Final（可扩展出“结构化工具调用事件”），见 [traits.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/zeroclaw/src/providers/traits.rs#L165-L177) 与 [traits.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/zeroclaw/src/providers/traits.rs#L491-L505)。
- `reasoning_content`：为 thinking models 的 round-trip fidelity 预留字段，避免 provider 因缺失字段拒绝历史，见 [traits.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/zeroclaw/src/providers/traits.rs#L68-L73)。

这套设计的核心是：把“工具 schema → provider 原生 payload”与“是否支持 native tool calling/streaming/vision”等差异封装在 provider 层，而不是散落在 agent loop 层。

## Python deepagents 如何调用 LLM

Python deepagents 的 LLM 调用不在 deepagents 自己实现；它依赖 LangChain 的 `BaseChatModel` 抽象与 LangGraph 的 runnable/graph。

### 模型解析与构图入口

在 [graph.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/deepagents/deepagents/graph.py)：

- `resolve_model(model)`：若是 `openai:` 前缀，默认 `use_responses_api=True`（Responses API），否则 `init_chat_model(model)` 交给 LangChain 解析，见 [graph.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/deepagents/deepagents/graph.py#L81-L105)。
- `create_deep_agent(...)`：把 `model: BaseChatModel`、tools、middleware 组装后调用 `langchain.agents.create_agent(...)` 返回 `CompiledStateGraph`，见 [graph.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/deepagents/deepagents/graph.py#L304-L316)。

### middleware 如何介入“模型调用前”

deepagents 的 middleware 通过 `wrap_model_call/awrap_model_call(request, handler)` 修改 `ModelRequest`，再调用 `handler(...)` 继续下游模型调用。以摘要中间件为例：

- 先做 token 估算/截断/是否需要 summary 的决策
- 再 `handler(request.override(...))` 触发真实的模型调用
- 若 `ContextOverflowError` 则走“summary→重试”的路径

见 [summarization.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/deepagents/deepagents/middleware/summarization.py#L864-L930)。

### subagent 的调用方式

subagent 不是直接调用模型，而是调用“子 runnable（子 agent 图）”，由 LangGraph/LangChain 在子图内部完成模型调用。`task` 工具里可以直接看到 `subagent.invoke/ainvoke`：

见 [subagents.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/deepagents/deepagents/middleware/subagents.py#L430-L465)。

## Python deepagents 的 streaming 如何实现

结论先行：Python deepagents 的 streaming 是“agent-level event streaming”，由 LangGraph 的 `agent.astream(...)` 驱动，deepagents-cli/TUI/ACP 消费事件流并渲染；deepagents 本体不实现 LLM SSE。

### 事件流入口与 chunk 结构

TUI（Textual）消费 streaming 的核心循环：

- 调用：`agent.astream(stream_input, stream_mode=["messages","updates"], subgraphs=True, ...)`
- chunk 是一个 3 元组：`(namespace, mode, data)`

见 [textual_adapter.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/cli/deepagents_cli/textual_adapter.py#L605-L619)。

其中：

- `mode=="messages"`：`data=(message, metadata)`（文本、tool_call、ToolMessage 等）。
- `mode=="updates"`：`data` 为 dict（interrupt/todo 等状态更新）。

### 为什么过滤 subgraph 输出

TUI 默认只展示主图（空 namespace），忽略 subagent 并行输出，避免多路 token 混杂；subagent 的结果应通过 `task` 工具返回给主 agent 再展示。

见 [textual_adapter.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/cli/deepagents_cli/textual_adapter.py#L620-L683)。

### 文本 streaming：AIMessageChunk 的增量 text

TUI 从 `message.content_blocks` 中识别 `type=="text"`，持续追加到 UI 消息组件，形成逐段流式渲染：

见 [textual_adapter.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/cli/deepagents_cli/textual_adapter.py#L800-L834)。

### 工具调用 streaming：tool_call_chunk 的 buffer/组装

TUI 会处理 `content_blocks` 中的 `tool_call_chunk/tool_call`：

- `args` 可能是 dict（一次到齐）或 str（JSON 分片），str 会累积拼接并尝试 `json.loads`。
- 解析成功后，先 flush pending 文本，再 mount 一个 ToolCallMessage，并把 `tool_call_id` 关联到该 UI 组件，等待 ToolMessage 回填状态/输出。

见 [textual_adapter.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/cli/deepagents_cli/textual_adapter.py#L841-L938)。

### 工具结果回填：ToolMessage 更新 ToolCallMessage

当工具执行完成，LangGraph 产出 `ToolMessage`（含 tool_call_id/status/content），TUI 更新对应的工具卡片为 success/error，并展示输出；文件操作还会生成 diff 卡片。

见 [textual_adapter.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/cli/deepagents_cli/textual_adapter.py#L729-L770)。

### HITL/AskUser：updates 流里的 __interrupt__

deepagents 的“暂停等待用户输入/审批”不是靠解析文本，而是走 LangGraph interrupt。TUI 从 `updates` 流识别 `__interrupt__`，分别处理 ask_user 与 HITL 请求，并在 stream 结束后用 `Command(resume=...)` 恢复执行。

见 [textual_adapter.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/cli/deepagents_cli/textual_adapter.py#L631-L666)。

### 摘要 streaming 的隐藏策略

摘要中间件会触发额外的模型调用并产生 stream chunk；TUI 通过 metadata 识别 `lc_source="summarization"` 并过滤掉摘要模型的 token 流，只通过 spinner/通知向用户反馈摘要发生。

见 [textual_adapter.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/cli/deepagents_cli/textual_adapter.py#L689-L714)。

## 对齐讨论：是否有必要对齐？风险与问题

### 什么时候有必要对齐 zeroclaw 的能力

在以下需求同时存在时，“引入 LLM Provider 层（zeroclaw 风格）”的收益显著：

- 需要复用/统一多家 LLM 的工具调用格式与能力差异（native tool calling/vision/streaming tool events）。
- 需要 thinking models 的 reasoning 字段 round-trip（否则某些 provider 会拒绝 tool-call history）。
- 需要 provider-level streaming event（TextDelta/ToolCall/Final）以支持更细粒度的 UI/agent 体验。
- 需要 prompt-guided fallback 机制，让“不支持 native tools”的 provider 仍能工作。

### 主要风险

- 职责混淆：把 capabilities/convert_tools/stream_chat 等“通讯层责任”塞进 DeepAgentsRS 的 step Provider，会让同一个 trait 既承担 agent policy 又承担 HTTP/格式适配，导致扩展与测试困难。
- 语义缺口：DeepAgentsRS 的 `ProviderStep` 当前无法表达“一次模型响应同时包含文本与 tool_calls”，这与原生 tool calling provider 的语义不完全一致。
- API 破坏：给 `ProviderStep`/`ProviderRequest`/`Message` 增字段或改枚举会影响大量 match 与序列化；需要版本化或向后兼容策略。
- ToolSpec 不完整：DeepAgentsRS 的 `ToolSpec` 当前只有 name/description（缺参数 schema），原生工具调用适配会受限，见 [runtime/protocol.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/runtime/protocol.rs#L12-L16)。
- streaming 与 HITL/工具回填耦合：Python deepagents 的 streaming 是“事件流”，DeepAgentsRS 是“step + 副作用（执行工具）”。要对齐体验，需要引入 event 层或单独 runner，而不是简单加一个 `supports_streaming()`。

## 推荐实现思路（面向 DeepAgentsRS）

### 目标：能力对齐，但不混淆层次

建议保持 DeepAgentsRS 当前 step-based Agent Provider（对工具回填、HITL、中间件链非常友好），并新增一个 LLM 客户端层 trait（可借鉴 zeroclaw）：

- `LlmProvider`（通讯/适配）：`capabilities()`、`convert_tools()`、`chat()`、`stream_chat()`、usage、reasoning_content 等。
- `AgentProvider`（控制流）：保留当前 `Provider::step(...) -> ProviderStep`。

通过一个适配器把 `LlmProvider` 接到 `AgentProvider` 的 loop：

- “step 适配器”：一次 `chat(...)` 得到文本/工具调用后，映射到 `ProviderStep`（必要时扩展新的 step variant）。
- “event 适配器”：若要对齐 Python streaming，则提供 `run_stream(...) -> StreamEvent`，将 “文本 delta / tool_call / tool_result / interrupt / final” 作为稳定事件协议输出给 UI/CLI/ACP。

### 关键结构性改造建议（从低风险到高风险）

1. 补齐 ToolSpec schema：将 `ToolSpec` 扩展出 parameters/input_schema（默认 `{}`），为 native tool calling 与 prompt-guided 文本协议奠基。
2. 明确 capabilities 所在层：capabilities 应属于 LLM Provider 层，不建议塞进 Agent Provider。
3. reasoning_content 的 round-trip：若目标 provider 需要，建议在消息/状态中提供可回填通道。
4. 扩展 step 语义或引入 ProviderV2：用于表达“文本 + 工具调用同一响应”的原生语义，避免额外 step 调用导致成本与行为变化。
5. streaming 采用独立 runner：避免把事件流强行塞进现有的 step loop；先做 `stream_run`，稳定后再考虑统一。

## 对 Rust 版 streaming 的启示（与 Python 对照）

Python deepagents 的 streaming 不是“LLM token 流”本身，而是“agent-level 事件流”：

- 文本 delta（TextDelta）
- 工具调用 delta/开始（ToolCall / ToolCallChunk）
- 工具结果（ToolResult / ToolMessage）
- interrupt（HITL/ask_user）
- final

如果 DeepAgentsRS 目标是对齐 Python 的 UI/CLI/ACP 体验，建议把 runtime 对外输出抽象成事件流接口，而不是只暴露 step-based `ProviderStep`。step-based loop 仍可作为内部执行模型，event stream 作为外部消费协议。

## 附：关键代码入口索引

- DeepAgentsRS Provider 协议：[provider/protocol.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/provider/protocol.rs)
- DeepAgentsRS run loop：[runtime/simple.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/runtime/simple.rs)
- DeepAgentsRS 工具调用归一化：[runtime/tool_compat.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/runtime/tool_compat.rs)
- DeepAgentsRS tool_calls 修补：[runtime/patch_tool_calls.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/runtime/patch_tool_calls.rs)
- DeepAgentsRS provider timeout/cache：[runtime/prompt_cache_runtime.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/runtime/prompt_cache_runtime.rs)
- zeroclaw LLM Provider traits：[zeroclaw traits.rs](file:///Users/bytedance/Documents/Dev/deepagents-rs/zeroclaw/src/providers/traits.rs)
- Python deepagents 构图与模型解析：[python graph.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/deepagents/deepagents/graph.py)
- Python deepagents TUI streaming 消费：[textual_adapter.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/cli/deepagents_cli/textual_adapter.py)
- Python deepagents 非交互 streaming 消费：[non_interactive.py](file:///Users/bytedance/Documents/Dev/deepagents-rs/deepagents/libs/cli/deepagents_cli/non_interactive.py)

