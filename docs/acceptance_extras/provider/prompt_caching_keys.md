---
title: Extras Provider E2E - Prompt Caching Key 策略（分层 key 与命中解释）
scope: extras
---

## 1. 为什么要把“key 策略”单独验收

Prompt caching 最常见的线上问题不是“没命中”，而是：

- 命中边界不一致：同样的请求有时命中有时 miss
- 命中不可解释：无法说明为什么命中/为什么不命中
- key 过于粗糙：不同 tools/system 被错误复用，导致语义错误
- key 过于细：几乎永远 miss，缓存形同虚设

因此 key 策略必须可观测、可解释、可演进，且必须通过端到端验收固定下来。

## 2. 分层 key 模型（推荐作为默认）

把一次模型请求拆成三个可控层级（名称可调整但必须固定）：

- L0：Provider 层固定项（model_id、provider_name、temperature、tool_calling_mode 等）
- L1：稳定前缀（system + tools schema + 固定 runtime 配置）
- L2：动态后缀（当前轮 messages、最新用户输入、线程摘要/压缩事件等）

对应两种缓存模式（二选一，必须固定并文档化）：

- 模式 A（分层缓存，推荐）：
  - 缓存 L1 的编译/预处理结果（例如 prompt 编译、tokenization、provider 端 prompt cache id）
  - L2 仍走完整请求，但可复用 L1 的缓存产物
- 模式 B（全量缓存，简单但风险高）：
  - 缓存 L1+L2 的整体请求（命中率低但实现简单）

无论选择哪种，都必须能通过 events/metrics 输出：

- `cache_level: L1|L2|none`
- `cache_key_hash: ...`
- `cache_key_components: {model, tools_hash, system_hash, messages_hash}`（仅 hash）

## 3. 必须纳入 key 的输入集合

以下输入变化必须导致 key 变化（否则存在错误复用风险）：

- model/provider 变化（L0）
- tools schema 变化（L1）
- system prompt 变化（L1）
- tool_choice/并行工具策略变化（L0 或 L1，取决于实现）
- summarization event 变化（L2：因为模型可见 messages 前缀变化）

以下输入变化不应导致 L1 变化（否则过度细化）：

- 用户最新消息内容（应只影响 L2）
- tool result 内容（除非被写进 system/tools；通常应仅影响 messages）

## 4. E2E 场景（key 策略必测）

### PK-01：同一 system/tools，不同用户消息 → L1 命中，L2 不命中（分层模式）

给定：

- caching=on，分层模式 A
- Run1：system/tools 固定，User="a"
- Run2：system/tools 固定，User="b"

当：执行 Run1 与 Run2

则：

- Run2 的 L1 命中（cache_level=L1 hit）
- L2 不命中（或不存在 L2 cache）
- 输出可解释：tools_hash/system_hash 相同，messages_hash 不同

### PK-02：tools schema 变化必须导致 L1 miss

给定：

- Run1：tools=[t1]
- Run2：tools=[t1,t2]

当：执行

则：

- Run2 L1 miss（tools_hash 不同）

### PK-03：system 变化必须导致 L1 miss

给定：

- Run1：system="A"
- Run2：system="B"

则：

- Run2 L1 miss（system_hash 不同）

### PK-04：summarization event 变化必须导致 L2 变化

给定：

- Run1：无 `_summarization_event`
- Run2：有 `_summarization_event`（effective messages 前缀改变）

当：执行

则：

- 若存在 L2 cache：Run2 L2 miss（messages_hash 或 event_hash 不同）
- 若仅有 L1 cache：L1 可继续命中，但 L2 变化必须在解释字段中可见

### PK-05：key 解释必须可重建（debug 可诊断）

给定：

- 任意一次 run

当：读取 events 中的 cache 事件

则：

- 能看到 system_hash/tools_hash/messages_hash 的组合
- 能解释命中来自哪个 level

### PK-06：脱敏：组件 hash 不得可逆、不得含原文

给定：

- system/messages/tools 中含敏感串 "SECRET_TOKEN_ABC123"

当：执行并产生 cache 事件

则：

- events/metrics/落盘缓存文件中不出现该敏感串
- 只出现 hash

## 5. 通过标准

- PK-01/02/03/05/06 必须通过
- PK-04 在实现 summarization 后必须通过（否则缓存会对长对话不稳定）

