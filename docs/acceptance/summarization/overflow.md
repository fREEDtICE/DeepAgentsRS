---
title: Core Summarization E2E - ContextOverflow 回退链路
scope: core
---

## 1. 端到端效果

当模型请求因为上下文过长失败（ContextOverflow/ContextLengthExceeded 等），系统必须具备回退链路：

- 捕获该错误
- 触发 summarization（生成 `_summarization_event`，可选落盘）
- 重新构造更短的模型请求并重试

端到端目标是“不中断用户任务”，并且整个链路可诊断。

## 2. 验收环境

- ScriptedModel 支持在特定轮次模拟抛出 ContextOverflow
- backend=CompositeBackend（conversation_history 路由到 tempdir_history）
- 固定 thread_id="e2e_thread"

## 3. E2E 场景（必测）

### SF-01：一次 overflow → summarization → retry 成功

给定：

- 第 1 次模型调用抛 ContextOverflow
- 重试时 ScriptedModel 返回 Assistant("ok")（无 tool_calls）

当：运行 Runner

则：

- events 中出现：
  - ModelRequestBuilt(1)
  - ModelError(ContextOverflow)
  - SummarizationTriggered(reason=overflow)
  - ModelRequestBuilt(retry)
  - AssistantMessage("ok")
  - RunFinished(no_tool_calls)
- final_state._summarization_event 存在

### SF-02：overflow 下仍应遵循落盘失败不阻断原则

给定：

- conversation_history 写入失败
- 仍触发 overflow fallback summarization

当：运行 Runner

则：

- event 仍生成但 file_path 为空
- retry 仍发生

### SF-03：连续 overflow 的终止策略必须固定

给定：

- 连续 N 次（例如 3 次）retry 仍 overflow

当：运行 Runner

则：

- 系统在达到上限后终止，并给出明确终止原因（overflow_retry_limit）
- events 中可定位每次 retry 的原因与次数

## 4. 通过标准

- SF-01 ~ SF-03 全通过

