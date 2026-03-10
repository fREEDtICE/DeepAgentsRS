# Summarization 语义摘要：技术方案与迭代计划

## 结论（对应 ISSUE_LOG_RUST_VS_PYTHON.md L38-39）

[`ISSUE_LOG_RUST_VS_PYTHON.md#L38-L39`](../ISSUE_LOG_RUST_VS_PYTHON.md#L38-L39) 指出的缺口成立：DeepAgentsRS（Rust）当前 Summarization 的“摘要生成”仍是 **preview 拼接**，并未调用模型生成语义摘要，因此在长对话中对“信息保真 + 压缩效果”的能力上限明显低于 Python（deepagents）常见做法。

Rust 侧 Summarization 已具备若干关键基础设施（自动触发、cutoff、history 落盘、event 写入、`compact_conversation` 手动工具），缺口主要集中在“摘要生成器（summarizer engine）”这一块：需要把 `build_summary_message()` 从“拼接 preview”升级为“可插拔 summarizer”，并提供基于 provider 的 LLM summarizer（同时保留无 provider 时的降级）。

---

## 现状盘点（Rust vs Python）

### Rust（DeepAgentsRS）当前能力

- 自动触发与裁剪：`before_provider_step` 里按 `max_char_budget/max_turns_visible/min_recent_messages` 判定是否需要 summarization，并计算 cutoff，写入 `_summarization_event`，随后把“summary_message + cutoff 之后的消息”作为 effective messages 送入下一次 provider 调用。  
  - 入口：[summarization_middleware.rs:L132-L215](../../crates/deepagents/src/runtime/summarization_middleware.rs#L132-L215)
- 手动触发工具：支持 `compact_conversation`（由 runtime middleware 拦截并执行），用于模型/外部显式压缩会话。  
  - 工具处理：[summarization_middleware.rs:L217-L301](../../crates/deepagents/src/runtime/summarization_middleware.rs#L217-L301)
- history 落盘：把被裁剪掉的消息与摘要写到 `/conversation_history/<thread_id>.md`（可追溯）。  
  - store：[summarization_middleware.rs:L67-L111](../../crates/deepagents/src/runtime/summarization_middleware.rs#L67-L111)
- 工具参数裁剪：在 summarization 判定前对 tool args 做字符级裁剪，避免旧工具参数爆炸。  
  - 调用：[summarization_middleware.rs:L140-L147](../../crates/deepagents/src/runtime/summarization_middleware.rs#L140-L147)
- 摘要生成（缺口点）：`build_summary_message()` 只是抽取前 4 + 后 2 条 message 的 preview，并限制到 `max_summary_chars`；不具备语义抽取/归纳能力。  
  - 现实现：[summarization_middleware.rs:L354-L387](../../crates/deepagents/src/runtime/summarization_middleware.rs#L354-L387)

### Python（deepagents）常见做法（参考）

- 自动 summarization 会真正调用模型生成 summary，并与 history offload 结合；同时提供 `compact_conversation` 工具作手动触发。  
  - 实现概览：[summarization.py](../../../deepagents/libs/deepagents/deepagents/middleware/summarization.py)
- 触发策略更偏 token-aware：基于 `model.profile.max_input_tokens` 做 fraction trigger/keep，并在发生 `ContextOverflowError` 时回退到 summarization 重试。  
  - 默认策略：[summarization.py:L163-L200](../../../deepagents/libs/deepagents/deepagents/middleware/summarization.py#L163-L200)  
  - overflow 回退：[summarization.py:L917-L924](../../../deepagents/libs/deepagents/deepagents/middleware/summarization.py#L917-L924)

---

## 目标与非目标

### 目标

- 在 Rust 中提供**可插拔 summarizer**：默认保持现有 preview summarizer（零依赖、可离线），可选启用 LLM summarizer（基于 provider）
- 让 LLM summarizer 在不改变现有“触发/裁剪/落盘/event”语义的前提下，产出**稳定、可复现、结构化的摘要文本**
- 明确降级与安全边界：provider 不可用/失败/超时时回退到 preview summarizer；摘要不包含敏感信息；摘要长度可控

### 非目标（首期不做）

- 不在首期实现“严格 token 精确计数”的通用计数器（当前 provider API 未暴露稳定 token 计数/usage）
- 不把 summarization 变成一个可被任意工具调用的通用服务（先聚焦 runtime 内部的 summarization 链路）

---

## 关键设计点（Rust 特性与当前架构约束）

### 1) 不修改 RuntimeMiddleware trait 的前提下，怎么调用模型生成摘要

当前 `RuntimeMiddleware::before_provider_step` 没有 provider 句柄，但 CLI 装配路径同时持有 provider 与 middleware 构造点（见 [main.rs:L540-L561](../../crates/deepagents-cli/src/main.rs#L540-L561)）。因此可以通过“依赖注入”解决：

- 为 `SummarizationMiddleware` 增加一个可选的 `summarizer` 字段（trait object）
- LLM summarizer 内部持有 `Arc<dyn Provider>`（或者单独的 summarization provider 实例）
- `before_provider_step` 触发 summarization 时，调用 `summarizer.summarize(pruned_messages)` 获取摘要文本

这不会改变 runtime middleware 接口，也不要求 runtime/runner 重构。

### 2) 防止递归与不受控 tool use

LLM summarizer 调用 provider 时必须保证：

- **不走 runtime loop**（只做一次 provider.step）
- **不提供 tool_specs/skills**（防止 summarizer 触发工具调用，导致复杂递归）
- 使用专门的系统提示约束输出格式与长度

### 3) 摘要消息的 role 与可识别性

Rust 当前用 `role="user"` + `name="summarization"` + `SUMMARY_MESSAGE_V1` marker（见 [summarization_middleware.rs:L378-L386](../../crates/deepagents/src/runtime/summarization_middleware.rs#L378-L386)）。这与 Python 用 HumanMessage 的习惯一致，可保留；但建议把 marker 升级为 `SUMMARY_MESSAGE_V2`，以区分“语义摘要”与“preview 拼接摘要”，便于观测与迁移。

---

## 技术方案（推荐）

### A) 引入 Summarizer 抽象

新增 trait（概念）：

- `Summarizer::summarize(messages, max_chars) -> SummaryText`
- 两个实现：
  - `PreviewSummarizer`：复用现有 `build_summary_message()` 的逻辑（当前行为不变）
  - `ProviderSummarizer`：基于 `Provider` 做一次总结调用，输出结构化摘要文本

建议把“摘要文本”与“摘要消息 Message 构造”分离：

- summarizer 只负责 `String`
- middleware 负责组装 `Message { role,name,marker,content }` 并做 `max_summary_chars` 裁剪

### B) ProviderSummarizer 的请求格式（模型提示）

建议固定 prompt 模板，要求输出为可复现的 markdown 结构，避免自由发挥导致不可控漂移：

```text
你是会话压缩器。请基于给定的消息列表生成语义摘要，用于下一轮对话继续推理。

硬性要求：
1) 不要编造未出现的信息；不确定就写“未知/未提及”
2) 保留决定、约束、文件路径、关键输出、未解决问题
3) 不要包含任何密钥/令牌/密码（即使看到了也要用 [REDACTED]）
4) 输出必须不超过 {max_chars} 字符

输出格式（必须严格遵守）：
SUMMARY_MESSAGE_V2
## Goals
- ...
## Decisions
- ...
## Current State
- ...
## Next Actions
- ...
## Open Questions
- ...
```

消息输入建议采用“角色前缀 + 内容”的纯文本串（而不是原样 JSON），并沿用现有 `truncate_tool_args` 的裁剪结果，避免把巨型工具参数喂给 summarizer。

### C) 失败与降级策略

- provider 超时/错误：回退到 `PreviewSummarizer`
- provider 返回非 `FinalText/AssistantMessage`：回退
- 生成摘要超过 `max_summary_chars`：强制裁剪并追加 `...(summary truncated)...`（与现有一致）

### D) 观测与可回归性

在 `SummarizationEvent` 中补齐观测字段（建议）：

- `summary_kind: "preview" | "llm"`
- `summary_chars`
- `provider_error_code/provider_error_message`（若发生降级）

并保持 history 落盘语义不变：仍写入完整 pruned messages + summary 结果，便于离线诊断。

---

## 迭代计划（建议拆为 S-1 ~ S-4）

### S-1：抽象落地与兼容门禁

- 增加 `Summarizer` trait 与 `PreviewSummarizer`，不改变默认行为
- 将现 `build_summary_message()` 迁移为 `PreviewSummarizer` 实现或内部 helper
- 单测：确保现有 summarization 行为完全一致（golden）

### S-2：ProviderSummarizer（LLM 语义摘要）+ 单测闭环

- 实现 `ProviderSummarizer`，注入 `Arc<dyn Provider>`（可与主 provider 共用或单独实例）
- mock provider 脚本化测试：
  - 触发 summarization 后 summary_message 包含 `SUMMARY_MESSAGE_V2`
  - provider 失败时回退到 preview

### S-3：预算策略与鲁棒性增强

- 增加“字符预算 → token 预算”的可插拔计数器接口（先保留默认字符预算）
- 增加 provider error code 识别（如未来真实 provider 用 `context_overflow`），支持“溢出回退 summarization 并重试”

### S-4：真实 provider 集成后对齐 Python 体验

- 接入至少一个真实 provider 后：
  - 回归“长对话压缩后仍能保留关键决策/路径/待办”的 e2e 用例
  - 对齐 Python 的“fraction trigger/keep”体验（基于模型 profile 的 max_input_tokens）

---

## 验收标准

- 默认配置下：Summarization 行为与当前一致（只做 preview 拼接）
- 启用 LLM summarizer 后：摘要内容结构稳定（V2 模板），能保留关键决策/约束/路径/未解决问题
- provider 异常时：自动降级，不影响主对话继续推进
- history 落盘与 `_summarization_event/_summarization_events` 记录完整且可复现

