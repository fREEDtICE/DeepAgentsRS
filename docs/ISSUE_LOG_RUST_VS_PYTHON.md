# Rust 版 vs Python 版：问题记录

本文档用于记录在对比以下两套实现时，Rust 版本相对 Python 版本暴露出的能力缺口与潜在风险点。

- Python：`../../deepagents/`
- Rust：`../`

更新时间：2026-03-10

另见：
- [PYTHON_PARITY_MATRIX.md](acceptance/PYTHON_PARITY_MATRIX.md)

## 更新快照（对齐进展）

本节用于记录“相对本文档初版结论”的最新进展，避免读者误以为所有问题仍处于同一状态。初版问题条目保留不删，仅在此处说明哪些已补齐、哪些仍缺失、哪些新增风险点需要跟进。

### 已明显补齐/对齐的能力

- 默认中间件顺序：Rust CLI 的默认装配已通过 `RuntimeMiddlewareAssembler` 以 slot 固化，并与 Python `create_deep_agent()` 的主 agent 默认顺序对齐（TodoList / Memory / Skills / FilesystemRuntime / Subagents / Summarization / PromptCaching / PatchToolCalls）。  
  - 装配入口：[main.rs:L479-L574](../crates/deepagents-cli/src/main.rs#L479-L574)  
  - slot/排序规则：[assembly.rs](../crates/deepagents/src/runtime/assembly.rs)
- HITL 交互闭环：Rust 已提供 `ResumableRunner` 的“中断→approve/reject/edit→继续”闭环，并在 CLI interactive 模式下提供 stdin 交互实现。  
  - runner 测试：[hitl_phase10.rs](../crates/deepagents/tests/hitl_phase10.rs)  
  - CLI 交互循环：[main.rs:L593-L689](../crates/deepagents-cli/src/main.rs#L593-L689)
- 大工具输出落盘（offload）：Rust 已在 runtime 执行阶段支持“工具输出过大 → 写文件 → 用预览+引用替换消息内容”的机制，语义上对齐 Python 的 `/large_tool_results/<id>` 思路。  
  - SimpleRuntime：[simple.rs:L568-L679](../crates/deepagents/src/runtime/simple.rs#L568-L679)  
  - ResumableRunner：[resumable_runner.rs:L741-L852](../crates/deepagents/src/runtime/resumable_runner.rs#L741-L852)
- read_file 多模态：Rust 已支持图片读取并以 base64 image content block 返回。  
  - 测试：[multimodal_read_file_phase7.rs](../crates/deepagents/tests/multimodal_read_file_phase7.rs)

### 仍然未补齐的关键缺口

- 对外 API 主入口：`DeepAgent.run()` 仍为空实现；主路径依赖 runtime/runner。  
  - [agent.rs:L48-L52](../crates/deepagents/src/agent.rs#L48-L52)
- 真实 Provider 生态：Rust 仍以 mock provider 为主，缺少 OpenAI/Anthropic 等真实接入。
- PromptCaching：Rust 目前为占位统计实现，尚未实现缓存命中/复用语义（方案与迭代拆分见 [PROMPT_CACHING_PLAN.md](iteration/PROMPT_CACHING_PLAN.md)）。  
  - [prompt_caching_middleware.rs](../crates/deepagents/src/runtime/prompt_caching_middleware.rs)
- Summarization 语义摘要：Rust 仍以 preview 拼接为主，未做 LLM 语义总结（与 Python 常见做法仍有差距，方案见 [SEMANTIC_SUMMARIZATION_PLAN.md](iteration/SEMANTIC_SUMMARIZATION_PLAN.md)）。  
  - [summarization_middleware.rs:L322-L347](../crates/deepagents/src/runtime/summarization_middleware.rs#L322-L347)

### 新增/需要重点复核的风险点

- FilesystemRuntimeMiddleware 的事件可能不反映真实 offload：middleware 写入的 `_filesystem_runtime_event` 统计事件与 runtime 实际 offload 的执行点分离，若用于观测可能出现“offload 已发生但事件未标记”的错配风险。  
  - [filesystem_runtime_middleware.rs:L89-L120](../crates/deepagents/src/runtime/filesystem_runtime_middleware.rs#L89-L120)
- middleware 组装顺序的直觉陷阱：`RuntimeMiddlewareAssembler` 在同 slot（尤其 `User`）会按 label 字典序排序，而不是插入序，可能与集成方直觉不一致。  
  - [assembly.rs](../crates/deepagents/src/runtime/assembly.rs)

## 1) 对外 API / 产品形态缺口

### 1.1 对外 API 的主入口不完整

Rust 侧 `DeepAgent.run()` 目前是空实现；真正可用的对话循环在 `SimpleRuntime`。对集成方而言，这是明显的产品/API 缺口：调用者需要自行理解并组装 runtime/provider/middlewares，而不是面向一个稳定的 agent 主入口。

- 相关代码
  - [agent.rs:L41-L45](../crates/deepagents/src/agent.rs#L41-L45)
  - [simple.rs:L95-L203](../crates/deepagents/src/runtime/simple.rs#L95-L203)

### 1.2 缺真实 Provider 生态

Rust 核心库仅包含 mock provider，没有 OpenAI/Anthropic 等真实接入；Python 版可天然复用 LangChain 生态。

- 相关代码
  - [provider/mod.rs:L1-L4](../crates/deepagents/src/provider/mod.rs#L1-L4)

## 2) 上下文治理缺口

### 2.1 Summarization 更像“片段预览拼接”，不是语义摘要

Rust 的 `build_summary_message()` 只是抽取少量消息做 preview 拼接，并非通过模型进行语义总结；在长对话中对“信息保真”和“压缩效果”的能力弱于 Python 版常见做法。

- 相关代码
  - [summarization_middleware.rs:L322-L347](../crates/deepagents/src/runtime/summarization_middleware.rs#L322-L347)

### 2.2 缺少“超大工具输出自动落盘引用”机制

Python 版会在工具输出过大时，将内容写入 `/large_tool_results/{tool_call_id}`，并把 tool message 替换为“预览 + 文件引用”，以抑制上下文膨胀。Rust 版当前缺少同类机制。

- 相关代码（Python）
  - [filesystem.py:L1128-L1234](../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L1128-L1234)

## 3) HITL 能力缺口

### HITL 只有策略层，缺交互闭环

Rust 中 `ApprovalDecision::RequireApproval` 在 runtime 内会直接返回 tool error，不会“暂停→询问→恢复”。这和 Python CLI 的交互式 HITL loop 是两种产品形态。

- 相关代码（Rust）
  - [simple.rs:L680-L762](../crates/deepagents/src/runtime/simple.rs#L680-L762)
- 对比参考（Python CLI）
  - [non_interactive.py:L68-L86](../../deepagents/libs/cli/deepagents_cli/non_interactive.py#L68-L86)

## 4) 可能的 bug / 行为坑

### 4.1 normalize_messages() 的 JSON 误判风险

只要 assistant message 的 `content` 恰好是 JSON object，就可能被当作“包裹式 tool schema”去抽取字段，导致消息被意外改写。模型输出结构化 JSON 很常见，此处属于高频踩坑点。

- 相关代码
  - [tool_compat.rs:L29-L45](../crates/deepagents/src/runtime/tool_compat.rs#L29-L45)

### 4.2 空文件 read 的语义污染

`LocalSandbox.read()` 对空文件返回固定字符串 `"System reminder: File exists but has empty contents"`。这会把“系统提示样式文本”当成真实文件内容喂给模型，容易诱发幻觉式推断与错误决策。

- 相关代码
  - [local.rs:L98-L122](../crates/deepagents/src/backends/local.rs#L98-L122)

### 4.3 大文件读取的资源风险

`read_to_string + content.lines().collect()` 会把整个文件读入内存再分页切片；对超大文件或二进制误读会带来显著的内存与延迟风险。

- 相关代码
  - [local.rs:L107-L122](../crates/deepagents/src/backends/local.rs#L107-L122)

### 4.4 Summarization 摘要消息 role 固定为 user

`build_summary_message()` 固定返回 `role="user"` 的摘要消息；在不同 provider/提示工程下可能改变模型行为，并且与常见的 system/assistant 摘要习惯不一致。

- 相关代码
  - [summarization_middleware.rs:L339-L346](../crates/deepagents/src/runtime/summarization_middleware.rs#L339-L346)

### 4.5 “grep”语义与直觉不一致

Rust 的 `grep` 实现是逐行 `contains`（字面量匹配），不是 regex/rg 语义；若上层提示词或用户以为支持正则/上下文行等，会反复试错、浪费回合。

- 相关代码
  - [local.rs:L244-L295](../crates/deepagents/src/backends/local.rs#L244-L295)
