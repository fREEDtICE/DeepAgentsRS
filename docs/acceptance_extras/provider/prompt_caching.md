---
title: Extras Provider E2E - Prompt Caching（提示缓存）
scope: extras
---

## 1. 端到端效果（为什么这算“验收”）

Prompt caching 的目标不是“更快”这么泛，而是可判定的行为：

- 启用 caching 后，同一类请求（同一 system/tools/固定前缀）能产生缓存命中
- 命中时不会改变模型输出语义（同脚本输入 → 同输出；真实模型下至少不降低正确性）
- 禁用 caching 时不会误报命中
- 缓存不会泄露敏感信息：不会把 secrets/raw 文件内容写入可持久化缓存（或必须可配置为内存/短期）
- 缓存失效策略固定且可诊断（TTL/最大条目/命中率统计）

注意：在确定性 ScriptedModel 下，“输出一致性”天然成立，因此必须引入“缓存层可观测信号”来做端到端断言。

## 2. 可观测指标（必须）

要让 E2E 可判定，Provider 层必须暴露以下至少一种观测面：

- events 中的 `ProviderCacheEvent`：
  - `lookup_hit: bool`
  - `cache_key_hash: string`（只允许 hash，不允许原文）
  - `cache_backend: memory|disk|remote`
- 或 metrics 导出：
  - `prompt_cache_hit_total`
  - `prompt_cache_miss_total`
  - `prompt_cache_entry_count`

并且需要保证这些指标不包含原始 prompt/tools 文字。

## 2.1 Key 策略（分层缓存的验收入口）

本文件关注“命中/失效/脱敏”等端到端行为；缓存 key 如何分层、如何解释命中边界在独立文档中验收：

- [prompt_caching_keys.md](prompt_caching_keys.md)

## 3. 验收环境

- Case A：caching=off
- Case B：caching=on（memory backend）
- Case C（可选）：caching=on（disk backend，带 TTL）

模型：

- 确定性 E2E：ScriptedModel + “缓存包装层”仍参与 lookup/insert（即：即使模型返回固定值，cache 仍按真实逻辑运行）
- 真实模型冒烟：真实 provider + 小规模请求（不作为硬门槛）

## 4. E2E 场景（必测）

### PC-01：关闭 caching 时无命中

给定：

- caching=off
- 连续运行两次完全相同的 agent run（system/messages/tools 一致）

当：执行 Run1 与 Run2

则：

- 两次都产生 cache miss（或无 cache 事件）
- hit_total==0

### PC-02：开启 caching 后第二次命中

给定：

- caching=on（memory）
- 连续运行两次完全相同的 agent run

当：执行 Run1 与 Run2

则：

- Run1：miss 并 insert
- Run2：hit
- events/metrics 可判定 hit==1

### PC-03：请求变化导致 miss（key 稳定性）

给定：

- caching=on
- Run1 与 Run2 仅改变 tools schema（例如增加一个新工具）

当：执行 Run1 与 Run2

则：

- Run2 必须 miss（因为 tools 变化会改变 key）
- 命中不能跨不同 tools 集合

### PC-04：只改变“用户最新消息”不应复用 system 前缀缓存（按实现策略）

给定：

- caching=on
- Run1 与 Run2 system/tools 相同，但用户最后一条消息不同

当：执行

则（二选一，必须固定并文档化）：

- 方案 A：仍可能命中“system/tools 前缀缓存”，但最终请求的整体缓存 key 不命中（推荐：分层缓存）
- 方案 B：整体 key 不命中（简单实现）

无论哪种，都必须能在 events 中解释“命中发生在何处”。

### PC-05：TTL 过期（仅 disk/带 TTL 后端）

给定：

- caching=on（disk，TTL=1s）
- Run1 执行后等待 2s 再 Run2

当：执行

则：

- Run2 miss（过期）
- events 记录 eviction/expired（如果实现）

### PC-06：容量上限与逐出策略（LRU 等）

给定：

- caching=on，max_entries=2
- 依次执行 3 个不同 key 的 runs：A、B、C
- 再次执行 A

当：执行

则：

- A 可能被逐出（取决于策略），但行为必须固定且可诊断
- events/metrics 能看到 eviction_count 变化（若实现）

### PC-07：脱敏：cache_key 只允许 hash，不允许原文

给定：

- system/messages/tools 中包含特征敏感串 "SECRET_SHOULD_NOT_LEAK"

当：开启 caching 并执行

则：

- 任何 cache 事件/日志/落盘文件不包含该敏感串
- 若发现泄露即失败

### PC-08：并发一致性（可选但建议）

给定：

- 并发启动两个完全相同的 run

当：同时执行

则：

- 不应出现竞态导致的崩溃
- 命中/插入计数符合预期（允许出现 1 次重复 insert，但必须固定并文档化）

## 5. 通过标准

- PC-01 ~ PC-03、PC-07 必须通过
- PC-04 ~ PC-06 取决于实现策略，但一旦声明支持就必须通过
- 所有诊断信息必须脱敏
