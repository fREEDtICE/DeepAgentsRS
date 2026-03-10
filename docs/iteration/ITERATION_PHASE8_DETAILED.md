# Phase 8：SummarizationMiddleware（历史压缩）详细迭代计划

## 背景与目标

Phase 8 的目标是提供可插拔的历史裁剪/摘要能力，并与 Python deepagents 的可观察行为对齐。核心结果是：在不破坏 tool call 协议与可恢复性的前提下，控制上下文长度、提高长期会话稳定性，并把被裁剪的历史落盘以便审计与后续恢复。

### 关键对齐点（必须满足）

- `_summarization_event`：通过事件改写模型可见的 messages
- 历史落盘：裁剪内容保存到 `/conversation_history/{thread_id}.md`
- 裁剪 tool args：老的 write/edit 等工具 args 在摘要时需要裁剪
- tool call 语义不破坏：裁剪后仍能保证 tool call 回放一致性
- 大工具结果 offload 预留兼容：与 `/large_tool_results/...` 引用模板可组合

## 范围与非目标

### 范围

- SummarizationMiddleware 的 trait/API 设计
- 三类裁剪策略：按 token/字符预算、按轮次、按重要性
- `_summarization_event` 语义与消息改写流程
- 历史落盘路径规范与格式
- 工具 args 裁剪规则与安全约束
- 可观察输出与调试信息结构化输出

### 非目标

- 真实 LLM 摘要质量优化（可使用 mock 或可插拔 provider）
- 复杂记忆召回策略（交由 Phase 7/后续）
- UI 或可视化

## 设计原则

- 先接口后实现：trait 清晰、可替换
- 不破坏工具语义：裁剪不改变 tool call 的 call_id、name、顺序
- 可审计与可恢复：落盘后可定位原始内容
- 与现有中间件兼容：Filesystem/Skills/Subagents/ACP/CLI 不需改动即可接入

## 核心架构

### 数据流（高层）

1. Runtime 产出 messages + tool calls
2. SummarizationMiddleware 评估裁剪策略
3. 生成 `_summarization_event`，更新模型可见 messages
4. 被裁剪历史写入 `/conversation_history/{thread_id}.md`
5. 若存在 tool args 裁剪，写入可恢复引用
6. 下游继续执行或返回最终响应

### 组件边界

- `SummarizationMiddleware`：策略选择与执行入口
- `SummarizationPolicy` trait：预算计算与裁剪决策
- `SummarizationStore` trait：落盘与查询
- `SummarizationEvent`：消息改写描述结构

## 详细设计

### 核心 trait 设计（建议）

- `SummarizationPolicy`
  - `evaluate(context) -> Decision`
  - `Decision` 包含：是否摘要、裁剪范围、目标预算、重要性标签
- `SummarizationStore`
  - `persist(thread_id, summary, pruned_messages, metadata) -> Result<Ref>`
  - `load(ref) -> Result<Content>`
- `SummarizationMiddleware`
  - `apply(messages, tool_calls, state) -> (messages, events, state_delta)`

### `_summarization_event` 结构

- `event_type = "_summarization_event"`
- `thread_id`
- `summary`：摘要文本
- `pruned_range`：裁剪范围（message 索引、时间、轮次）
- `storage_ref`：`/conversation_history/{thread_id}.md` 中对应段落或标记
- `tool_args_redaction`：裁剪哪些 tool args 的索引与替换模板

### 历史落盘格式

- 路径：`/conversation_history/{thread_id}.md`
- 内容结构：
  - 元信息区：thread_id、时间、策略、预算、哈希
  - 原始消息区（裁剪段）
  - 摘要区
  - tool args 裁剪映射区（原始 args 哈希与引用）

### tool args 裁剪规则

- 仅裁剪已完成且不再影响上下文推理的历史 tool args
- 保留 `tool_call_id`、`name`、`result` 结构
- 对 `write_file/edit_file` 等工具的 args 使用占位模板：
  - `{"redacted": true, "ref": "...", "hash": "..."}`
- 不裁剪当前轮或最近 N 轮的 tool args

### 策略细节

#### 1) 预算策略（token/字符）

- 输入：`max_token_budget` 或 `max_char_budget`
- 触发条件：当模型可见消息超过预算
- 裁剪对象：最旧的用户/助手消息优先
- 约束：不裁剪与未完成 tool call 相关消息

#### 2) 轮次策略

- 输入：`max_turns_visible`
- 触发条件：轮次超限
- 裁剪对象：最早轮次
- 约束：保留系统提示与运行时重要指令

#### 3) 重要性策略

- 输入：重要性评分与阈值
- 触发条件：评分低的历史段落累积超限
- 裁剪对象：低重要性内容优先
- 约束：安全与策略相关消息不能裁剪

### 与其他中间件协作

- FilesystemMiddleware：确保 read/write/edit 的 state 更新不被摘要破坏
- PatchToolCallsMiddleware：确保 tool_call_id 规范化后再裁剪
- SubAgentMiddleware：子代理历史不继承 parent，摘要仅作用于当前 runtime
- SkillsMiddleware：技能调用的 schema 不变，args 裁剪仅影响历史
- ACP/CLI：不需要改协议，只接收最终消息与事件

## 配置与默认值

- `summarization.enabled = true`
- `summarization.policy = budget`
- `summarization.max_char_budget = 12000`
- `summarization.max_turns_visible = 12`
- `summarization.min_recent_turns = 3`
- `summarization.history_path = /conversation_history/{thread_id}.md`
- `summarization.redact_tool_args = true`

## 错误处理与安全

- 落盘失败：降级为不摘要并返回警告事件
- 裁剪失败：保持原消息，写入 error code
- 未知策略：返回可观测错误，禁止静默失败
- 权限与路径：必须走 Backend 的安全路径校验
- 不记录敏感信息：对敏感字段做 hash + ref

## 可观察输出与调试

- 在运行输出中增加 `summarization_events` 字段
- 记录策略选择、裁剪范围、落盘引用
- 提供调试模式：输出裁剪前后长度对比

## 测试计划

### 单元测试

- 策略触发：预算、轮次、重要性
- tool args 裁剪模板正确性
- `_summarization_event` 结构与序列化
- 落盘格式与读取回放

### 集成测试

- 带工具调用对话：裁剪后仍可继续执行
- 多轮对话：裁剪后模型可见消息符合预算
- 落盘文件可被读取并复现上下文

### E2E（建议）

- `deepagents run ...` 触发摘要并在输出中包含事件
- ACP 会话中触发摘要且不破坏会话关闭

## 验收标准

- 裁剪后不破坏 tool call 可恢复性
- `_summarization_event` 可观测且格式稳定
- `/conversation_history/{thread_id}.md` 落盘正确
- tool args 裁剪符合模板与安全要求
- 回归测试全部通过

## 风险与对策

- 路径模型未冻结：落盘路径可能与虚拟路径冲突
  - 对策：Phase 8 明确采用虚拟路径或后端真实路径映射
- 裁剪误伤 tool 语义
  - 对策：严格保留 tool_call_id 与 result 结构
- 摘要质量不足
  - 对策：先提供可替换策略与 provider，后续优化

## 待办事项（Phase 8）

- 定义 SummarizationPolicy 与 SummarizationStore trait
- 实现 SummarizationMiddleware 的空实现与事件框架
- 实现预算/轮次/重要性三类策略
- 实现历史落盘与读取回放
- 实现 tool args 裁剪模板与规则
- 补齐单测与集成测试
- 完成 E2E 覆盖与文档
