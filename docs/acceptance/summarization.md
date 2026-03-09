---
title: Core 验收 - Summarization（_summarization_event / 历史落盘 / compact）
scope: core
---

## 1. 能力定义（E2E 效果）

Summarization 的端到端效果不是“把 messages 真的删掉”，而是：

- 在 state 中写入 `_summarization_event`，并在下一次模型调用前把 state.messages 映射成 effective messages：
  - effective = `[summary_message] + messages[cutoff_index..]`
- 把被逐出的历史追加落盘到 `/conversation_history/{thread_id}.md`
- 支持链式压缩：多次 summarization 时 cutoff 折算正确，不丢消息、不重复摘要
- 支持手动 `compact_conversation` 工具（与自动 summarization 共用 event 机制）

参考 Python 实现： [summarization.py](../../../deepagents/libs/deepagents/deepagents/middleware/summarization.py)。

## 2. 对外契约（必须对齐）

### 2.1 state key

- `messages`：完整对话历史（不一定被物理裁剪）
- `_summarization_event`：私有事件（cutoff_index、summary_message、file_path）

### 2.2 effective messages 计算

- 无事件：effective == messages
- 有事件：effective == `[summary_message] + messages[cutoff_index..]`

### 2.3 链式 cutoff 折算（关键）

如果已有 prior event，则新的 cutoff 可能基于 effective 索引产生，需要折算回 state 索引：

`state_cutoff = prior_cutoff + effective_cutoff - 1`

这条必须通过 E2E 断言验证。

### 2.4 落盘路径与追加语义

- 目标路径：`/conversation_history/{thread_id}.md`
- 写入方式：追加 markdown section（不能覆盖旧历史）
- 需要过滤“摘要消息”避免 summary-of-summary 重复落盘（Python 用 additional_kwargs.lc_source=="summarization" 标记）
- 落盘失败不应阻断 summarization；event.file_path 可为 None

### 2.5 手动工具：compact_conversation

- 工具无参数
- 若对话未达到 eligibility gate，应返回明确提示“还不需要 compact”
- 若执行，必须更新 `_summarization_event` 并产出一条 ToolMessage 确认

## 3. 验收环境

- backend=CompositeBackend：
  - `/conversation_history/` → FilesystemBackend(tempdir_history)
  - default → FilesystemBackend(tempdir_workspace)
- 固定 thread_id="e2e_thread"
- 提供可控触发机制（至少一种）：
  - 通过调小 trigger 阈值触发
  - 或 ScriptedModel 模拟 ContextOverflow 触发 fallback summarization

## 4. E2E 场景（Summarization 必测）

### S-01：第一次 summarization 生成 event 与落盘

给定：

- 构造 messages 足够长，使自动 summarization 在某轮触发（阈值调小或脚本触发）

当：运行 Runner 触发 summarization

则：

- final_state._summarization_event 存在且包含：
  - cutoff_index（>0）
  - summary_message（可判定为摘要类型）
  - file_path=="/conversation_history/e2e_thread.md"（或等价虚拟路径）
- tempdir_history 下生成 `conversation_history/e2e_thread.md`，包含被逐出的历史（append section）

### S-02：effective messages 真的影响下一轮模型输入

给定：

- 在 S-01 之后继续运行下一轮
- ScriptedModel 在该轮断言：其输入 messages 的第 1 条必须是 summary_message，且其余从 cutoff_index 开始

当：Runner 构造下一轮模型请求

则：

- 断言成立（证明不是“写了 event 但没生效”）

### S-03：链式 summarization（cutoff 折算正确）

给定：

- 让对话继续增长，触发第二次 summarization
- ScriptedModel 断言第二次 summarization 前后的 cutoff 折算满足：
  - 新的 state cutoff = prior_cutoff + effective_cutoff - 1

当：触发第二次 summarization

则：

- event.cutoff_index 单调递增（不会回退）
- 落盘文件追加新 section（不会覆盖旧 section）
- effective messages 不会出现“重复摘要消息堆叠”（只能有 1 条 summary_message 作为 effective[0]）

### S-04：ContextOverflow fallback（模型调用溢出时仍能 summarization）

给定：

- ScriptedModel 在某轮模拟抛出 ContextOverflow

当：Runner 捕获并走 summarization fallback

则：

- 系统能生成 `_summarization_event` 并继续运行到收敛（或至少能继续到下一轮请求）
- events 中能定位到 “overflow → summarization → retry” 的链路

### S-05：compact_conversation eligibility gate

给定：

- 对话很短（低于 eligibility 阈值）
- 模型调用 compact_conversation()

当：执行工具

则：

- 返回明确提示“不满足 compact 条件”
- `_summarization_event` 不应变化
- 不应写入 conversation_history

### S-06：compact_conversation 成功路径（更新 event + 落盘 + ToolMessage）

给定：

- 对话达到 eligibility 阈值（阈值可调小）
- 模型调用 compact_conversation()

当：执行工具

则：

- `_summarization_event` 更新
- conversation_history 文件追加 compact 相关 section（标题可不同，但必须可判定为一次新追加）
- 返回 ToolMessage 表示 compact 完成

### S-07：落盘失败不阻断 summarization

给定：

- 注入一个会让 `/conversation_history/` 写入失败的 backend（例如只读、或故意返回错误）
- 触发 summarization

当：执行 summarization

则：

- summarization 仍返回成功（event 仍生成）
- event.file_path == None（或空）
- 事件流包含可诊断的“落盘失败”信息（但不泄露敏感路径）

### S-08：旧工具参数截断（降低 token，不等同 summarization）

给定：

- 历史中包含旧的 write_file/edit_file tool_calls，参数 content 极长
- 开启 truncate_args_settings，使其在某轮触发

当：Runner 构造模型请求

则：

- 旧消息中的长参数被截断为 “前缀 + truncation_text”
- 最近的消息窗口内不应被截断（keep 生效）
- 截断只影响模型可见内容，不应改变真实文件副作用或 state 中保存的真实文件内容

### S-09：避免 summary-of-summary 重复落盘

给定：

- 已经生成过 summary_message
- 继续触发 summarization/compact

当：落盘被逐出历史

则：

- conversation_history 中不应重复保存先前的 summary_message（只保存原始对话片段）
- 可通过在 summary_message 里放入独特标记并断言落盘文件不包含该标记来验证

## 5. 通过标准

- S-01 ~ S-09 全通过
- 每次 summarization/compact 都能在 artifacts 中定位到：
  - final_state.json 中的 `_summarization_event` 变化
  - conversation_history 文件内容追加
  - events.jsonl 中的触发原因与链路

