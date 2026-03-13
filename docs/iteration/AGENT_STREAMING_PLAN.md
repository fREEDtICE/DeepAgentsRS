# 技术方案与迭代计划 - AgentProvider 对外 Streaming（Runner 事件流）

## 背景

当前 Rust 版 DeepAgents 已具备 step-based 的 `Provider` 与 runtime 闭环：

- `Provider::step(req) -> ProviderStep`
- runtime 负责工具执行、ToolMessage 回填、state 合并、interrupt/resume

但对外仍以一次性 `RunOutput` 为主，缺少稳定的 agent-level streaming 接口。现有文档和验收已经明确要求 Runner 产出结构化事件流，用于 CLI/UI/ACP/测试消费：

- [Core 验收 - Runner](./../acceptance/runner.md)
- [Core Runner E2E - 事件流与可断言性](./../acceptance/runner/events.md)
- [HITL interrupt/resume 方案](./HITL_INTERRUPT_RESUME_PLAN.md)
- [Provider 能力对齐研究](./../RESEARCH_PROVIDER_ALIGNMENT.md)

本方案的目标是：在不破坏当前 step-based Provider 与 runtime 语义的前提下，为 DeepAgents 增加稳定、可测试、可扩展的对外 streaming 能力。

## 关键结论

1. `AgentProvider` 不应直接扩展为“流式 provider 协议”。
2. 对外 streaming 的正确抽象层是 Runner/Runtime，而不是 ChatModel / LLM Provider。
3. 第一期先实现 agent-level 事件流，不要求底层 LLM provider 支持 token streaming。
4. 第二期再引入可选的 `LlmProvider::stream_chat()`，把更细粒度的 text/tool delta 映射到统一的 Runner 事件流。
5. `ResumableRunner` 应作为唯一执行内核，`SimpleRuntime` 最终收敛为对其的 collect 包装，避免双实现分叉。

## 设计目标

### 必须满足

- 对外提供稳定的结构化事件流
- 与现有 `RunOutput` 结果保持一致
- interrupt/resume 是一等事件，而不是错误
- 事件顺序固定，可做 golden snapshot
- 不要求所有 provider 原生支持 token streaming
- 不泄露真实路径与敏感数据

### 非目标

- 第一阶段不要求实现 token 级 LLM SSE
- 第一阶段不重构为 LangGraph 式通用图引擎
- 第一阶段不改现有 `Provider::step(...)` 契约

## 参考基线：LangChain 非 legacy 的做法

参考对象限定为 LangChain v1 / LangGraph 心智，不包含 legacy agent executor。

### 值得借鉴的点

1. streaming 在 agent graph / runtime 层实现，而不是在 chat model 层实现  
   参考：`create_agent(...).stream(..., stream_mode="updates" | "messages")`
2. 请求对象采用不可变模式  
   参考：`ModelRequest.override(...)`
3. middleware 语义分层清晰  
   参考：`before_model / after_model / wrap_model_call / wrap_tool_call`
4. state 更新显式建模  
   参考：model 与 middleware 最终都归一为 `Command(update=...)`
5. interrupt 是控制流  
   参考：HITL 在 `after_model` 处理 tool calls，而不是通过异常中断整个 agent

### 不直接照搬的点

- 不引入 LangGraph 风格的通用图执行框架
- 不使用弱类型的 `stream_mode="messages"/"updates"` 作为 core 协议
- Rust 侧优先使用强类型 `RunEvent`，而不是 tuple/dict 事件

## 总体架构

### 分层

#### 1. Agent Provider（控制流层）

保持现状：

- 输入：`ProviderRequest`
- 输出：`ProviderStep`

职责：

- 决定本轮输出 assistant 文本 / tool calls / skill call / final / error

不负责：

- HTTP 通讯
- provider-specific stream 协议
- 对外事件流

#### 2. Runner Core（执行层）

统一由 `ResumableRunner` 承担：

- 构造 provider request
- 调用 provider
- patch/normalize provider step
- 执行工具
- 回填 `ToolMessage`
- 合并 state
- 处理中断与恢复

#### 3. Streaming Facade（对外协议层）

新增统一事件协议：

- `RunEvent`
- `RunEventSink`

职责：

- 把 Runner 内部每个关键状态转移暴露给外部消费者
- 为 CLI / ACP / TUI / 测试提供稳定消费接口

#### 4. Optional LLM Streaming Adapter（后续扩展层）

后续新增独立 `LlmProvider`：

- `chat()`
- `stream_chat()`
- `capabilities()`
- `convert_tools()`

通过适配器把 provider-level delta 转为 `RunEvent`。

当前状态：

- `LlmProvider::capabilities()` 已落地，当前至少声明 streaming、tool calling、usage、structured output、reasoning content 五类能力

## 核心接口设计

### 1. 事件协议

建议新增：`crates/deepagents/src/runtime/events.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    RunStarted {
        resumed_from_interrupt: bool,
    },
    ModelRequestBuilt {
        step_index: usize,
        tool_names: Vec<String>,
        skills: Vec<String>,
        message_count: usize,
        message_summary: Vec<MessageSummary>,
    },
    ProviderStepReceived {
        step_index: usize,
        step_type: ProviderStepKind,
    },
    AssistantTextDelta {
        step_index: usize,
        text: String,
    },
    AssistantMessage {
        step_index: usize,
        message: Message,
    },
    ToolCallStarted {
        step_index: usize,
        tool_name: String,
        tool_call_id: String,
        arguments_preview: serde_json::Value,
    },
    ToolCallArgsDelta {
        step_index: usize,
        tool_call_id: String,
        delta: String,
    },
    ToolCallFinished {
        step_index: usize,
        tool_name: String,
        tool_call_id: String,
        output_preview: serde_json::Value,
        error: Option<String>,
        status: Option<String>,
    },
    ToolMessageAppended {
        step_index: usize,
        tool_call_id: String,
        content_preview: String,
        status: Option<String>,
    },
    StateUpdated {
        step_index: usize,
        updated_keys: Vec<String>,
    },
    Interrupt {
        step_index: usize,
        interrupt: HitlInterrupt,
    },
    Warning {
        step_index: usize,
        code: String,
        message: String,
    },
    UsageReported {
        step_index: usize,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        total_tokens: Option<u64>,
    },
    RunFinished {
        status: RunStatus,
        reason: String,
        final_text: String,
        step_count: usize,
        tool_call_count: usize,
        tool_error_count: usize,
    },
}
```

### 2. Sink 接口

先采用 sink，而不是直接在 core trait 暴露 `Stream<Item = RunEvent>`：

```rust
#[async_trait]
pub trait RunEventSink: Send {
    async fn emit(&mut self, event: RunEvent) -> anyhow::Result<()>;
}
```

初始实现建议包括：

- `NoopRunEventSink`
- `VecRunEventSink`
- `JsonlRunEventSink`
- `ChannelRunEventSink`

原因：

- 最小侵入现有 runtime
- 对象安全与生命周期更简单
- 测试可直接用 `VecRunEventSink`
- CLI/ACP 可先用 channel/jsonl 包装

### 3. Runtime 扩展接口

建议新增 trait，而不是直接破坏现有 `Runtime`：

```rust
#[async_trait]
pub trait StreamingRuntime: Send + Sync {
    async fn run_with_events(
        &self,
        messages: Vec<Message>,
        sink: &mut dyn RunEventSink,
    ) -> RunOutput;
}
```

说明：

- `StreamingRuntime` 面向 `SimpleRuntime` 这类 stateless runtime
- `ResumableRunner` 继续通过 inherent `run_with_events()` / `resume_with_events()` 提供 stateful streaming 能力

对于 resumable 场景，再补：

```rust
impl ResumableRunner {
    pub async fn run_with_events(
        &mut self,
        sink: &mut dyn RunEventSink,
    ) -> RunOutput;

    pub async fn resume_with_events(
        &mut self,
        interrupt_id: &str,
        decision: HitlDecision,
        sink: &mut dyn RunEventSink,
    ) -> RunOutput;
}
```

### 4. `SimpleRuntime` 的定位

中期目标：

- `SimpleRuntime::run()` 退化为对 `ResumableRunner` 的一次性 collect 包装

这样：

- 只维护一套 loop 语义
- 所有 run 路径天然具备 event 能力
- interrupt/resume 与 non-resumable run 不再分叉

## 事件语义与顺序约束

直接对齐现有 acceptance 文档，并补充 ProviderStep 适配细节。

### 1. 同一轮的固定顺序

最低固定顺序：

1. `ModelRequestBuilt`
2. `ProviderStepReceived`
3. `AssistantTextDelta*` 可选
4. `AssistantMessage` 或 `Interrupt`
5. 对每个 tool call：
   - `ToolCallStarted`
   - `ToolCallFinished`
   - `ToolMessageAppended`
   - `StateUpdated` 可选
6. `RunFinished` 若本轮终止

### 2. interrupt 边界

命中 interrupt 时必须满足：

- 发出 `Interrupt`
- 立即发出 `RunFinished { status: interrupted }`
- 不得出现 `ToolCallStarted`

该约束与现有 HITL 验收保持一致。

### 3. 可复现性

ScriptedModel / MockProvider 下，必须稳定：

- 事件数量
- 事件类型顺序
- 关键字段：`step_index`、`tool_name`、`tool_call_id`、`reason`

禁止在核心事件中注入 wall-clock 时间。

### 4. 隐私约束

事件不得泄露真实磁盘路径。

只允许：

- 虚拟路径
- 预览
- 摘要
- 哈希或引用

## 现有 ProviderStep 到 RunEvent 的映射

### `AssistantMessage { text }`

发出：

- `ProviderStepReceived`
- `AssistantMessage`

内部 effects：

- 追加 assistant message
- 继续下一轮

### `FinalText { text }`

发出：

- `ProviderStepReceived`
- `AssistantMessage`
- `RunFinished { status: completed, reason: "final_text" }`

说明：

- 最终文本也作为 assistant 输出暴露，避免流式消费者需要特殊处理

### `ToolCalls { calls }`

发出：

- `ProviderStepReceived`
- `AssistantMessage`，其中 message 含结构化 `tool_calls`
- 后续每个工具的开始/结束/回填事件

说明：

- 即使 provider 本身没有自然文本，也应向外发一条 assistant message，以固定事件心智

### Skills

Package skills 通过普通 `ToolCalls { calls }` 暴露给模型与事件流，不保留单独的
`SkillCall` 事件类型。

### `Error { error }`

发出：

- `ProviderStepReceived`
- `RunFinished { status: error, reason: "provider_step_error" }`

## 内部执行模型建议：引入 `StepEffects`

为减少 runtime 各处直接写副作用，建议在 Runner 内部引入归一化结构：

```rust
struct StepEffects {
    assistant_messages: Vec<Message>,
    tool_calls: Vec<ProviderToolCall>,
    tool_messages: Vec<Message>,
    updated_keys: Vec<String>,
    interrupt: Option<HitlInterrupt>,
    final_text: Option<String>,
    warnings: Vec<RunnerWarning>,
}
```

处理流程：

1. `ProviderStep -> StepEffects`
2. 先发事件
3. 再提交 effects 到 `messages/state/tool_results`

收益：

- 事件与实际提交的数据来源统一
- 便于测试
- 便于以后接入 provider-level delta

## 与 LangChain v1 对齐的具体借鉴点

### 1. 不可变请求对象

LangChain v1 的 `ModelRequest` 使用 `override()` 生成新请求，而不是原地修改。

Rust 建议：

- 为 `ProviderRequest` 补充 builder / copy-with 风格 API
- middleware 不直接原地改输入，而是返回新的 request/messages/state 视图

### 2. middleware 分层语义

LangChain v1 的 middleware 分为：

- `before_model`
- `after_model`
- `wrap_model_call`
- `wrap_tool_call`

Rust 当前已有：

- `before_run`
- `before_provider_step`
- `patch_provider_step`
- `handle_tool_call`

建议后续演进时向更明确的语义层靠拢，而不是新增一条专门的 streaming middleware 通道。

### 3. interrupt 是控制流

LangChain v1 的 HITL 在 `after_model` 检查 tool calls，再触发 interrupt 并重写消息。

Rust 侧应保持：

- `Interrupt` 是 event
- `RunFinished(interrupted)` 是可恢复终止
- 不将其编码为 error

### 4. state patch 显式化

LangChain v1 使用 `Command(update=...)` 作为统一 state/message 更新载体。

Rust 不必照搬 `Command`，但应有等价的 `StepEffects / StatePatch` 内部建模，避免“事件来自一套逻辑、实际提交来自另一套逻辑”。

## 与未来 `LlmProvider` 的关系

本方案不依赖 `LlmProvider` 才能落地，但必须为其预留扩展点。

### 第一阶段

无 `LlmProvider` 时：

- `AssistantTextDelta` 不产生
- 只产出 coarse-grained agent events

### 第二阶段

新增：

- `LlmProvider::stream_chat() -> Stream<LlmEvent>`

由 `LlmAgentProviderAdapter` 负责：

- 消费 `LlmEvent`
- 实时发 `AssistantTextDelta`
- 实时发 `ToolCallArgsDelta`
- 发 `UsageReported`
- 最终组装为稳定 `ProviderStep` 或 `StepEffects`

注意：

- 对外仍只暴露 `RunEvent`
- 不把 provider 私有 chunk 协议直接暴露给 CLI/ACP/UI

## 实现对齐审查（2026-03-11）

截至 `codex/feat/llmprovider` 当前实现，以下部分已经与本方案对齐：

- `ResumableRunner` 已成为 streaming 的主执行内核
- `SimpleRuntime` 已收敛为对 `ResumableRunner` 的薄包装
- `RunEvent` / `RunEventSink` 已落地
- CLI 已支持 `--events-jsonl` / `--stream-events`
- ACP 已支持 `/run_stream` / `/resume_stream`
- 已接入独立 `LlmProvider` 与 openai-compatible provider 骨架

但仍存在 4 个需要显式记录的偏差：

1. provider delta 不是实时输出，而是在 provider stream 完成后统一回放  
   这会让 `/run_stream` 与 CLI 事件流无法真正边生成边消费。
2. 普通 `run()` 路径已经被耦合到 `step_with_collector()`  
   这削弱了 `chat()` / `stream_chat()` 的职责分离。
3. L2 response cache 命中时不会重放 provider delta / usage  
   首次运行与缓存命中的流式外观不一致。
4. `ToolSpec` 仍缺少 `input_schema`  
   openai-compatible tool calling 目前只是协议通路，不是完整的 provider-native schema 实现。

### 对事件顺序约束的补充修订

coarse-grained provider 仍保持原顺序：

1. `ModelRequestBuilt`
2. `ProviderStepReceived`
3. `AssistantMessage` / `Interrupt`
4. tool events
5. `RunFinished`

但对支持 provider-native streaming 的实现，顺序需要修订为：

1. `ModelRequestBuilt`
2. `AssistantTextDelta*` / `ToolCallArgsDelta*` / `UsageReported*` 可选
3. `ProviderStepReceived`
4. `AssistantMessage` / `Interrupt`
5. tool events
6. `RunFinished`

原因：

- `ProviderStepReceived` 依赖最终组装出来的稳定 `ProviderStep`
- 实时 delta 必然先于最终 step 到达

因此，`ProviderStepReceived` 在 streaming provider 场景下应被视为“final step assembled”事件，而不是“第一块 provider 数据已到达”事件。

## 更新后的近期迭代计划

### Phase 3.5：修正 live delta 架构

目标：

- `run_with_events()`、CLI streaming、ACP SSE 真正边生成边输出 delta
- 不再先把 provider delta 缓冲到 `Vec` 再统一回放

范围：

- 新增直接转发到 `RunEventSink` 的 provider event collector
- `ResumableRunner::run_with_events()` 改为 live collector 路径
- `ProviderStepReceived` 改为在最终 step 组装完成后发出

验收：

- 首个 `AssistantTextDelta` 发生在 `AssistantMessage` 之前
- streaming provider 场景下，delta 不再晚于最终 step 的消费时机

### Phase 3.6：恢复 `chat()` / `stream_chat()` 职责分离

目标：

- `run()` / `/run` 使用稳定的非流式 provider 路径
- `run_with_events()` / `/run_stream` 才使用 streaming provider 路径

范围：

- `step_with_prompt_cache()` 保持 coarse path
- 新增面向 live collector 的 prompt cache 扩展入口
- `resume()` 与 `resume_with_events()` 同步收敛到相同原则

验收：

- 非流式运行不依赖 `stream_chat()`
- 流式运行可消费实时 delta

### Phase 4：定义缓存与 streaming 的契约

建议先明确以下规则：

- L2 response cache 命中只产出 coarse events，不重放 delta
- 文档和 acceptance 明确说明该差异

原因：

- provider delta 本身通常不是稳定可重放数据
- 强行缓存 delta 会增大状态体积，并带来顺序一致性问题

当前状态：

- 该契约已确定，并已有测试覆盖

### Phase 5：补齐 `ToolSpec.input_schema`

目标：

- 为 openai-compatible provider 生成真实 JSON Schema
- 让 provider-native tool calling 具备语义正确性

范围：

- 扩展 `ToolSpec`
- runtime 的内置 tools/skills tools 提供 schema
- OpenAI-compatible request builder 改用真实 schema

## 数据输出策略

### 预览字段

事件中的大字段必须走 preview：

- `arguments_preview`
- `output_preview`
- `content_preview`

策略：

- JSON object 仅保留前 N 个 key
- 大文本只保留头尾摘要
- 大工具结果复用 existing offload preview 逻辑

### metadata 约束

允许为外部 UI/调试保留少量 metadata，例如：

- `step_index`
- `lc_source` 等未来兼容字段

但不应引入 provider-specific 噪音字段作为 core 协议的一部分。

## 对现有 CLI / ACP / TUI 的影响

### CLI

建议新增：

- `--events-jsonl <path>`
- `--stream-events`

行为：

- 非交互模式可直接输出 JSONL 事件
- interactive 模式可边跑边渲染

### ACP

建议新增：

- SSE 或 chunked JSON 事件流接口

事件仍使用统一 `RunEvent`，避免 ACP 维护另一套事件枚举。

### TUI

未来 TUI 可以直接消费：

- `AssistantTextDelta`
- `ToolCallStarted`
- `ToolCallFinished`
- `Interrupt`
- `RunFinished`

无需感知 provider 是 step 模型还是 token streaming 模型。

## 风险与对策

### 风险 1：`SimpleRuntime` 与 `ResumableRunner` 双实现长期分叉

对策：

- 尽快将 `SimpleRuntime` 收敛为 `ResumableRunner` 包装

### 风险 2：事件协议过早暴露过多细节

对策：

- core 只暴露稳定且最小必要的事件类型
- provider 私有 streaming 细节只在 adapter 内部处理

### 风险 3：事件 payload 过大

对策：

- 全部使用 preview / summary
- 复用 large tool result offload 逻辑

### 风险 4：interrupt/resume 事件次序与现有 HITL 方案不一致

对策：

- 以 acceptance 文档为唯一真值
- 在实现前先冻结顺序约束

### 风险 5：直接暴露 `Stream<Item = RunEvent>` 导致对象安全与生命周期复杂

对策：

- 第一阶段以 `RunEventSink` 为核心接口
- 第二阶段在外围包装 channel stream / SSE

## 迭代计划

### Phase S0：契约冻结

目标：

- 冻结 `RunEvent` 协议、事件顺序、preview 规则、终止原因

交付物：

- 本文档
- 对 acceptance 文档的映射说明

验收：

- 文档评审通过
- 事件枚举无重大歧义

### Phase S1：事件协议与 sink 骨架

范围：

- 新增 `runtime/events.rs`
- 定义 `RunEvent` / `RunEventSink`
- 提供 `NoopRunEventSink` / `VecRunEventSink`

验收：

- 单测：事件可序列化/反序列化
- `VecRunEventSink` 可用于 golden snapshot

### Phase S2：`ResumableRunner` 接入 coarse events

范围：

- 在 `ResumableRunner::run()` 主循环埋点
- 覆盖：
  - `ModelRequestBuilt`
  - `ProviderStepReceived`
  - `AssistantMessage`
  - `ToolCallStarted`
  - `ToolCallFinished`
  - `ToolMessageAppended`
  - `StateUpdated`
  - `Interrupt`
  - `RunFinished`

验收：

- 对齐 [runner/events.md](./../acceptance/runner/events.md) 的 RE-01 ~ RE-06

### Phase S3：`SimpleRuntime` 收敛为包装层

范围：

- `SimpleRuntime::run()` 内部委托 `ResumableRunner`
- 移除双 loop 分叉

验收：

- 现有 runtime 测试无回归
- `run()` 与 `run_with_events(NoopSink)` 结果一致

### Phase S4：CLI / ACP 对外 streaming

范围：

- CLI 输出 `events.jsonl`
- ACP 暴露统一事件流接口
- non-interactive interrupt 时输出 machine-readable 事件与结果

验收：

- CLI / ACP 黑盒 E2E 可断言事件序列

### Phase S5：resume streaming 闭环

范围：

- `resume_with_events`
- pending interrupt 的幂等暴露
- 恢复后的事件续接

验收：

- 对齐 [HITL_INTERRUPT_RESUME_PLAN.md](./HITL_INTERRUPT_RESUME_PLAN.md)
- 无重复执行、无中断丢失

### Phase S6：细粒度 provider delta 接入

前提：

- 引入独立 `LlmProvider`

范围：

- `AssistantTextDelta`
- `ToolCallArgsDelta`
- `UsageReported`
- 可选 `ReasoningDelta`

验收：

- delta 折叠结果与最终 assistant message 一致
- 不支持 streaming 的 provider 自动降级

## 验收映射

本方案直接挂靠现有文档：

- Runner 闭环： [runner.md](./../acceptance/runner.md)
- 事件顺序： [events.md](./../acceptance/runner/events.md)
- HITL interrupt/resume： [HITL_INTERRUPT_RESUME_PLAN.md](./HITL_INTERRUPT_RESUME_PLAN.md)
- Provider 分层与 future `LlmProvider`： [RESEARCH_PROVIDER_ALIGNMENT.md](./../RESEARCH_PROVIDER_ALIGNMENT.md)

建议新增的补充验收：

- `run()` 与 `run_with_events()` 的输出一致性
- `events.jsonl` golden snapshot
- `Interrupt` 后不得出现 `ToolCallStarted`
- `resume_with_events()` 不得重复发已完成工具事件
- large tool result preview 与实际回填一致

## 推荐的近期执行顺序

1. 先冻结 `RunEvent` 协议
2. 在 `ResumableRunner` 实现 `run_with_events`
3. 用 `VecRunEventSink` 补齐 core acceptance
4. 收敛 `SimpleRuntime`
5. 再做 CLI/ACP 外部 streaming
6. 最后接入独立 `LlmProvider` 的细粒度 delta

## 结论

DeepAgents 的 AgentProvider 对外 streaming，最稳妥的实现路径不是把 `Provider::step()` 改造成流式 trait，而是：

- 保持 step-based Provider 不变
- 由 Runner 统一输出强类型 agent event stream
- 用 `ResumableRunner` 作为唯一执行内核
- 后续通过独立 `LlmProvider` 适配更细粒度 token/tool delta

这一路径与 LangChain 非 legacy 的分层思路一致，同时更符合 Rust 当前代码结构与 acceptance 基线。
