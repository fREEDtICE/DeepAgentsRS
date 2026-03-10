---
title: 技术方案与迭代计划 - PromptCaching（提示缓存）
scope: iteration
---

## 1. 问题背景

Rust 侧 `PromptCachingMiddleware` 目前仅为占位统计实现（对 `AgentState.extra` 计数），没有任何“缓存命中/复用”语义，也没有可判定的端到端观测面，因此无法满足对齐 Python 默认栈中 `AnthropicPromptCaching` 的目标，也无法通过仓库内既有的 prompt caching 验收用例。

- 现状实现：[prompt_caching_middleware.rs](../../crates/deepagents/src/runtime/prompt_caching_middleware.rs)
- 问题记录入口：[ISSUE_LOG_RUST_VS_PYTHON.md#仍然未补齐的关键缺口](../ISSUE_LOG_RUST_VS_PYTHON.md)
- 验收定义（必须可判定）：[acceptance_extras/provider/prompt_caching.md](../acceptance_extras/provider/prompt_caching.md)
- Key 策略验收（分层 key 与命中解释）：[acceptance_extras/provider/prompt_caching_keys.md](../acceptance_extras/provider/prompt_caching_keys.md)

### 1.1 当前系统现状（对方案的硬约束）

- Provider 生态：Rust 主路径仍以 `mock/mock2` 为主，短期内无法依赖真实 provider 回传的“缓存命中使用量”来做验收。
- Runtime 扩展点：`RuntimeMiddleware` 当前没有可包裹 provider 调用的钩子（只能 pre/patch），因此“真正的 lookup/short-circuit”不适合放在 middleware 内实现。
- 数据结构与输出：`RunOutput.trace` 是可扩展的 JSON 字段，但运行时在多个 early-return 分支会直接构造 trace；缓存事件需要能跨分支稳定注入。
- 依赖现状：`deepagents` crate 当前不包含哈希库、LRU 容器等依赖（见 [deepagents/Cargo.toml](../../crates/deepagents/Cargo.toml)），若要实现稳定 hash 与 LRU/TTL，需要新增依赖或自实现最小集合。

## 2. 目标与非目标

### 2.1 目标（必须满足）

- **命中/复用语义可判定**：开启 caching 后，在可控场景下（确定性 provider 或 mock）能稳定命中，并能解释“为什么命中/为什么 miss”。
- **可观测且脱敏**：至少提供一种观测面（events 或 metrics），且只允许输出 hash，不得输出原始 prompt/system/tools/messages 文字。
- **key 策略固定**：明确哪些输入会影响 L1/L2，并通过 E2E 固化（见 key 验收文档）。
- **默认安全**：默认关闭；启用后默认使用内存后端；落盘后端必须显式开启，并具备 TTL/容量上限与逐出策略。
- **与现有架构契合**：不强行把 provider 细节塞进 `Message.content`，避免污染 tool_compat 的 JSON 兼容逻辑。

### 2.2 非目标（本迭代不做）

- 不把 “prompt caching” 等同于 “response caching” 并默认开启（全量响应缓存可能改变随机采样语义，仅作为可选模式）。
- 不在没有真实 provider 的前提下承诺“真实模型 token 节省”效果；本阶段只保证语义闭环与可观测性。

## 3. 关键概念澄清：Prompt Caching vs Response Cache

仓库内同时存在两类相关能力：

- **Prompt caching（本方案）**：围绕一次 provider 请求的“提示前缀/结构”建立可复用的缓存产物，并提供命中解释（L0/L1/L2）。它可以是：
  - 真实 provider 支持的 prompt caching（例如 Anthropic 的 cache_control 机制）
  - 或本地层对请求进行“预处理/归一化结果”的缓存（用于可观测与测试）
- **Response cache（ZeroClaw 现有）**：把“完整输入→完整输出”落盘，避免重复调用模型：
  - [zeroclaw response_cache.rs](../../../zeroclaw/src/memory/response_cache.rs)

本方案优先落地 PromptCaching 的可观测闭环，并把“全量 response cache”作为可选实现/门禁扩展项，避免语义争议。

## 4. 设计总览

### 4.1 选型：把缓存放在 Provider 包装层，而不是 Middleware 里“硬拦截”

当前 `RuntimeMiddleware` 接口没有 `around_provider_step()` 这类能包裹 provider 调用的钩子；仅靠 `before_provider_step()`/`patch_provider_step()` 无法实现真正的 lookup/short-circuit。  
因此建议引入 **CachingProvider**（Provider wrapper）来负责：

- 计算 key
- lookup/insert
- 产出 `ProviderCacheEvent`

而 `PromptCachingMiddleware` 只承担“可配置开关注入/统计兜底/兼容性”工作，避免为缓存而扩展 middleware trait。

### 4.1.1 Rust 语言特性带来的实现要点

- 避免不必要 clone：`ProviderRequest` 体积较大（messages/state/tool_specs），CachingProvider 应尽可能基于引用计算 hash，并把缓存值以 `Arc<ProviderStep>` 存储以降低 clone 成本。
- 并发安全与粒度：ACP/CLI 可能并发多会话；缓存容器必须是线程安全的（例如 `Arc<Mutex<...>>` 或分段锁），并在锁粒度上避免把 provider 的真实调用包在大锁里。
- 可配置的隔离分区：默认建议按 `root` 或 `session_id` 做 cache partition，避免跨工作区/跨租户复用引发“信息侧信道”；需要共享时再显式开启。

### 4.2 Key 模型：分层（L0/L1/L2）+ 可解释组件 hash

按 [prompt_caching_keys.md](../acceptance_extras/provider/prompt_caching_keys.md) 推荐的分层 key 模型：

- **L0（Provider 固定项）**
  - provider 名称/实现版本（例如 `mock/mock2/openai/anthropic`）
  - model 标识（若存在）
  - 影响采样/工具调用的配置（temperature/tool_choice 等；没有则置空）
- **L1（稳定前缀）**
  - system prompt（或 system messages 合并后的 canonical 视图）
  - tools schema（`ToolSpec` + skills tools）
  - 固定 runtime 配置（影响 messages 形态的选项，如 summarization/on-off）
- **L2（动态后缀）**
  - 当前轮 messages（含最近用户输入、tool results）
  - summarization event（若它会改变“模型可见 messages 前缀”）

输出到事件中（只允许 hash）：

```json
{
  "type": "provider_cache",
  "cache_backend": "memory",
  "cache_level": "L1",
  "lookup_hit": true,
  "cache_key_hash": "…",
  "components": {
    "l0_hash": "…",
    "system_hash": "…",
    "tools_hash": "…",
    "messages_hash": "…"
  }
}
```

### 4.3 Canonicalization：保证同一输入得到稳定 hash

Rust 侧 `serde_json::Value`（尤其 object）存在 key 顺序不稳定问题；若直接 `to_string()` 再 hash，会导致“同输入偶发 miss”。  
建议采用**递归排序对象 key** 的 canonical JSON 序列化（或将 map 转换为 `BTreeMap`），再计算 SHA-256。

原则：

- hash 输入必须稳定
- 任何可观测输出只包含 hash，不包含原文

#### 哈希算法选择（Rust 侧建议）

- 推荐：SHA-256（稳定、实现成熟、碰撞风险低）。实现上建议新增 `sha2` 依赖（workspace 级统一版本）。
- 备选：BLAKE3（更快，但同样需要新增依赖）。
- 不建议：`std::collections::hash_map::DefaultHasher` 作为长期方案（非加密 hash、稳定性/跨版本承诺不足，不适合作为对外可观测 key_hash 的基础）。

### 4.4 可观测性落点

目标是让 E2E 可判定且不污染现有 public API：

- **方案 A（推荐，增量字段）**：在 `RunOutput.trace` 中追加 `provider_cache_events: [...]`（JSON），并保持原 trace 字段结构可兼容扩展：[protocol.rs](../../crates/deepagents/src/runtime/protocol.rs)
- **方案 B**：把 events 写入 `AgentState.extra["_provider_cache_events"]` 并在 `RunOutput.trace` 镜像输出（更接近 summarization 的现有做法）

两者都满足“脱敏 + 可判定”，建议选 A 以避免 state 体积膨胀。

#### 与现有 trace 结构的兼容方式（建议固定）

为避免 runtime 多处 early-return 导致事件丢失，建议采取以下固定策略之一：

- 策略 A：缓存事件暂存于 `AgentState.extra["_provider_cache_events"]`（仅存 hash 与元信息），最终在构造 `RunOutput.trace` 时统一镜像输出并清理；该策略对多分支最鲁棒。
- 策略 B：在 runtime/runner 内维护 `Vec<ProviderCacheEvent>` 并在每个 return 点注入到 trace；该策略侵入面更大但避免 state 膨胀。

本仓库已有“把诊断事件写入 state.extra 并在输出中暴露”的先例（summarization_events），因此更推荐策略 A。

### 4.5 缓存后端（分阶段）

- **Memory 后端（必做）**
  - LRU + TTL
  - 进程内共享（Arc<Mutex<..>> 或 dashmap）
- **Disk 后端（可选）**
  - SQLite（可复用 zeroclaw 的实践：WAL + TTL + LRU eviction）
  - 默认关闭，必须显式开启并配置 TTL/max_entries

### 4.6 “复用语义”的分阶段承诺

为避免把 “prompt caching” 退化成“全量 response cache”，建议分两条路径：

1) **L1 缓存（默认路径）**：缓存并复用“稳定前缀的 canonical 化与 hash 产物”，并在真实 provider 接入后扩展为“provider 端 prefix cache 复用”（例如 Anthropic cache_control 标注 + usage 观测）。
2) **L2 全量缓存（可选能力）**：对 `ProviderRequest` 的整体进行缓存，直接复用 `ProviderStep`（用于 mock/确定性 provider 的 token 节省与 E2E 可判定）。该能力必须：
   - 默认关闭
   - 明确声明“可能改变随机采样语义”（仅在确定性或 temperature=0 场景建议开启）

### 4.7 可增加的相关能力：PromptLayoutPolicy 与 payload 映射（让“前缀稳定”变成系统能力）

第 8~10 节补充的 prompt 工程策略，本质上要求系统能稳定产出：

- “可缓存前缀”（system + 可选 developer + tools schema）
- “动态后缀”（对话轮次与工具历史）

建议把这件事显式实现为一组可复用能力，避免每个 provider/调用方用隐式约定各做一套，导致：

- key_hash 稳定但实际 payload 不稳定（命中解释失真）
- 多 system/developer 的兼容性在不同网关下漂移（缓存 miss + 行为漂移）

可增加的能力点：

- **PromptLayoutPolicy（配置）**
  - `system_prefix`：长期稳定策略（尽量固定、可长）
  - `developer_prefix`：可选的“固定 developer”（若 provider 支持）
  - `task_instruction_role`：优先 developer，若不支持则退化为 system（禁止退化为 user）
  - `merge_system_strategy`：多 system 的确定性合并规则（固定顺序 + 固定分隔符）
- **ProviderPayloadMapper（映射）**
  - 输入：内部统一的 `Vec<Message>`（含 system/developer/user/assistant/tool）
  - 输出：面向“该 provider 实际发送”的 canonical payload view（可序列化、可 hash、脱敏）
  - 约束：key 的 `system_hash/messages_hash` 必须基于该 canonical payload view 计算，而不是基于“映射前的消息列表”
- **PromptStabilityDiagnostics（诊断）**
  - 在 provider cache events 中可选输出：system/developer 是否包含明显动态字段（时间戳、request id 等）的布尔标记或计数（不得输出原文）
  - 用于把“缓存 miss”从不可解释变为可诊断：是 input 真的变了，还是合并/映射规则不稳定

## 5. 端到端验收门禁（直接对齐现有文档）

以以下文档为硬门禁：

- [prompt_caching.md](../acceptance_extras/provider/prompt_caching.md)：PC-01/02/03/07 必须通过
- [prompt_caching_keys.md](../acceptance_extras/provider/prompt_caching_keys.md)：PK-01/02/03/05/06 必须通过（若声明支持分层模式 A）

最低可交付定义（MVP）：

- 支持 memory backend
- 输出 provider cache events（脱敏）
- L2 全量缓存可在 mock provider 下稳定命中（PC-02）
- tools/system 变化导致 miss（PC-03）

## 6. 迭代拆分（建议按依赖顺序）

### PC-1：定义数据结构与可观测事件（不引入缓存行为）

- 交付物
  - `ProviderCacheEvent`、`CacheLevel(L0/L1/L2)`、`CacheBackend` 数据结构
  - `RunOutput.trace` 扩展或 `AgentState.extra` 镜像输出策略
  - `PromptCachingMiddleware` 从“计数占位”升级为“配置注入 + 事件容器初始化”（仍不改变行为）
- 验收
  - JSON 序列化快照测试（确保字段名固定且不含原文）
  - PC-07（脱敏）在纯事件层可通过（构造包含敏感串的输入，断言 events 不含原文）

### PC-2：CachingProvider（memory）+ L2 全量缓存（用于可判定命中）

- 交付物
  - `CachingProvider<P: Provider>`：对 `ProviderRequest` 计算 canonical hash 并做 lookup/insert
  - memory LRU + TTL + max_entries
  - events：每次 lookup/insert 产生事件（hit/miss、key_hash、components hash）
  - mock provider e2e：相同请求第二次命中（PC-02）
- 验收
  - PC-01/02/03/07 通过
  - 并发稳定性基础用例（PC-08 可选）

#### PC-2 的 Rust 落地建议（降低依赖与复杂度）

- 容器：先实现“HashMap + LRU list（VecDeque）+ TTL 时间戳”的最小版本，避免引入过多新依赖；如后续需要性能再替换为成熟 LRU crate。
- 缓存值：建议缓存 `ProviderStep::FinalText/AssistantMessage/ToolCalls/Error` 的完整枚举，存为 `Arc<ProviderStep>`；命中时直接 clone Arc 并返回，避免深拷贝。
- 事件：命中与 miss 都要产出事件（否则 PC-01/02 难判定），且必须保证事件中不出现原文。

### PC-3：Key 分层（L1/L2）与命中解释（对齐 key 验收）

- 交付物
  - 生成并上报 `system_hash/tools_hash/messages_hash` 等组件 hash
  - 分层模式 A（推荐）：
    - L1：system+tools 的 hash/归一化结果缓存（即使复用收益小，也要保证命中边界可解释且稳定）
    - L2：整体请求缓存（可开关）
- 验收
  - PK-01/02/03/05/06 通过

### PC-4：Disk 后端（可选）+ TTL/逐出策略

- 交付物
  - SQLite 后端（WAL、TTL、LRU eviction）
  - eviction/expired 的事件或指标
- 验收
  - PC-05/06（若声明支持）通过

### PC-5：真实 Provider 对接（解锁“真正的 prompt caching”）

依赖：至少落地一个真实 provider（Anthropic/OpenAI）并提供 usage/metadata 回传。

- 交付物
  - Anthropic：在 provider 侧对 system/tools/messages 标注 cache_control，并从响应 usage 中提取 cache 相关统计，映射为 `ProviderCacheEvent`
  - OpenAI：若有等价机制则对齐；否则仅保留本地分层 key 与事件
- 验收
  - 真实模型冒烟：连续两次相同请求可观测到命中信号（不作为硬门槛，但作为 release 前门禁）

#### 与 Python 思路对齐但不照搬（Rust 侧差异点）

Python 的 `AnthropicPromptCachingMiddleware`（来自 `langchain_anthropic`）更偏向“在请求上标注 cache_control 以启用 provider 的 prompt caching”。Rust 侧要对齐其效果，需要满足两个前提：

- 具备真实 Anthropic provider（或等价 SDK 接入）并能控制请求 payload（插入 cache_control 等字段）
- 能拿到可判定的命中信号（usage/metadata 或 SDK 回传字段），并映射为脱敏事件/指标

在此之前，Rust 侧的“可判定闭环”应以 mock/确定性环境的可观测事件为主，先把 key/命中边界/脱敏固定下来，避免直接把实现绑死在某个 provider 的私有语义上。

### PC-6：PromptLayoutPolicy + provider 映射与探针（把第 8~10 节变成可执行能力）

- 交付物
  - `PromptLayoutPolicy`：system/developer 角色策略 + 多 system 合并策略（确定性）
  - `ProviderPayloadMapper`：至少为 mock provider 提供“最终 payload 视图”的构造（用于 hash 与 debug）
  - 探针用例（CLI 或 e2e test）：
    - developer role 支持性探针（支持则生效，不支持则降级到 system）
    - 多 system 合并一致性探针（等价于固定拼接）
    - 缓存命中边界探针（system/tools/messages 微改动对应 PK-* 结果）
    - 隔离维度探针（不同 session/workspace 不互相命中，除非显式共享）
- 验收
  - 探针用例全绿，并能从 `ProviderCacheEvent` 中解释命中边界与隔离维度（只输出 hash/标记）

## 7. 风险与权衡

- **语义风险**：L2 全量缓存可能改变随机采样输出；必须默认关闭并要求显式开启。
- **稳定性风险**：hash canonicalization 若不严格，会导致“同输入偶发 miss”，必须把 PK-* 作为硬门禁。
- **敏感信息风险**：任何事件/落盘文件不得包含原文；只允许 hash。PC-07 必须作为回归门禁长期保留。
- **指令层级风险**：为提高缓存命中而“把开发者指令塞进 user message”会降低指令优先级并扩大 prompt injection 面；应优先使用 system/developer 层承载内部约束。
- **隔离与侧信道风险**：自建缓存若未按 tenant/session/workspace 分区，可能产生跨会话命中、信息侧信道或缓存投毒；必须把隔离维度纳入 key 或 partition。

## 8. 工程实践补充：固定前缀与消息角色策略（面向 Prompt Cache）

本方案在 Rust 侧的 “prompt caching” 分层 key（L1/L2）与 canonicalization，天然要求我们对“哪些内容应该稳定、哪些内容必然变化”有清晰边界。实践上常见的工程策略是把输入拆成“可缓存前缀”与“动态后缀”，并把前缀尽可能固定以提高 provider 侧的前缀缓存（prefix/KV cache 或供应商 prompt cache）命中率。

### 8.1 典型消息布局（推荐范式）

若目标模型/API 支持开发者指令层（developer），推荐：

- system：固定且尽量长（安全策略、工具调用规约、输出格式基准、脱敏规则）
- developer：每次请求的“本次任务指令”（成功标准、输出结构、允许/禁止项），必要时再加一条“固定 developer”
- user/assistant/tool：多轮对话与工具调用历史

对应到本方案的分层 key：

- L1：system（或 system messages 的合并视图）+ tools schema + 与 messages 形态相关的固定运行时配置
- L2：本轮动态 messages（用户输入、工具输出、摘要事件等）

实践要点：

- 前缀可缓存要求 token 序列一致，因此 system/developer 中不要包含时间戳、请求 id、随机数、实验分流标记等动态字段。
- 尽量把“随任务变化但不影响安全边界”的内容放入 developer，而不是反复改动 system；system 作为更稳定的“底座策略”。

### 8.2 为什么“把指令做成 message 插入”能帮助缓存

多数推理侧缓存依赖“输入前缀 token 完全一致”。将固定策略集中在 system（以及可选的固定 developer）并保持完全不变，使得每次请求共享同一段前缀，从而：

- 降低首 token 延迟（避免重复计算长前缀）
- 在部分平台上降低计费 prompt tokens（取决于供应商是否对前缀缓存做计费优惠）

这与本方案的 key 设计是一致的：L1 尽量稳定、可解释；L2 明确承载动态信息。

### 8.3 多条 system message：可用但不应依赖“额外语义”

连续多条 system message 在模型层面通常等价于“按顺序拼接成一个更长的系统指令”。其价值主要是可维护性（模块化），而不是更强的遵循度。

工程风险在于“接口兼容性”：

- 部分 OpenAI-compatible 网关/代理可能只保留第一条或最后一条 system，或以非确定方式拼接，导致行为漂移与缓存 miss。
- 因此在跨供应商/兼容层场景，建议在发送前就把多条 system 做确定性合并（固定顺序与固定分隔符），并在 canonicalization 里同样采用一致的合并规则，保证 hash 稳定。

## 9. 供应商差异与能力映射（developer/system 多样性）

不同供应商对“系统/开发者指令”的接口抽象差异较大，直接影响“如何放置本次任务指令”以及“如何获得稳定前缀”。

### 9.1 OpenAI（developer role 原生）

OpenAI 的 Chat Completions 接口在 role 枚举中包含 `developer`，并定义了 developer message 作为“开发者提供、模型应遵循的指令”。在 o1 及更新模型的语义里，developer messages 用于替代之前 system messages 的同类用途。参见官方 API 参考：<https://platform.openai.com/docs/api-reference/chat/create>

落地建议：

- 把长期稳定策略放 system，把“每次任务指令”放 developer，减少对 system 的频繁修改，从而提高前缀缓存复用率与行为一致性。

### 9.2 Gemini / Anthropic 等（system 指令为独立字段或顶层参数）

部分 API 不使用 `messages[]` 里的 `system`/`developer` role 表达系统指令，而是通过独立字段承载（例如 `systemInstruction/system_instruction` 或顶层 `system` 参数）。这类 API 的“多条 system message”并非其原生语义，应在接入层做统一映射：

- 将 system/developer 的合并视图映射到供应商的系统指令字段
- 将 user/assistant/tool 映射到其对话内容结构

本方案的 key 组件拆分（system_hash/tools_hash/messages_hash）仍然适用，但 canonicalization 必须以“映射后的最终 payload 视图”为准，避免出现“本地 hash 稳定但实际发给 provider 的文本不稳定”的错位。

### 9.3 OpenAI-compatible 网关/自建推理（兼容性不确定）

对 OpenAI-compatible 端点应默认假设：

- 可能接受 `developer` role 但降级为 system 或 user（语义漂移）
- 可能对多条 system 处理不一致

因此需要用“探针用例”固化行为（见下一节），并在实现上尽量减少依赖这些不稳定语义：

- 多 system 先合并为一条
- developer 不可用时，退化为 system（而不是 user）

## 10. 验证清单：多 system / developer / 缓存命中是否真实生效

为避免“接口能接收但语义不生效”，建议在接入新 provider 或新网关时执行以下最小验证：

- **语法层验证**：发送包含多条 system、包含 developer 的请求，确认不触发 schema 校验错误。
- **语义层验证**：用 developer/system 写入强约束（例如“只输出 OK”），再给 user 输入诱导输出其他内容，检查是否仍遵循约束。
- **拼接一致性验证**：在 system 中放置可识别标记 A/B，多 system 分别包含 A、B，检查模型行为是否等价于“拼接后的指令”。
- **缓存命中验证**：同一请求连续两次调用，观测 `ProviderCacheEvent.lookup_hit=true` 且 key_hash 相同；对 system/tools/messages 逐一做微小改动，确认命中边界与事件解释符合 PK-* 门禁。
- **隔离验证**：不同 session/workspace/tenant 的同一输入不应互相命中（除非显式启用共享），并能在事件中解释 partition/隔离维度。
