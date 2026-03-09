---
title: Core Subagents E2E - 上下文隔离（messages 与私有 state）
scope: core
---

## 1. 端到端效果（必须硬隔离）

子 agent 只应该收到“任务描述”，而不是主线程的长对话历史。端到端效果需满足：

- child_state.messages 必须被重置为 `[Human(description)]`
- child_state 必须过滤掉一组私有/不定义合并语义的 keys

对齐 Python 的排除集合（证据）： [subagents.py:L115-L128](../../../../deepagents/libs/deepagents/deepagents/middleware/subagents.py#L115-L128)：

- messages
- todos
- structured_response
- skills_metadata
- memory_contents

## 2. 验收环境

- 主线程 state 预置多轮 messages（含特征串 "SECRET_IN_MAIN"）
- 主线程 state 同时预置：
  - skills_metadata={"x":"y"}
  - memory_contents="MEM"
  - todos=[{...}]
- 注册一个断言型子 agent（脚本化）：它会把自己收到的输入 messages 与 state keys 回显出来

## 3. E2E 场景（必测）

### SI-01：child messages 必须只有 1 条 Human(description)

给定：

- 主 messages 有 20 条
- description="check"

当：调用 task(description="check", subagent_type="assertor")

则：

- 子 agent 回显其输入 messages 长度为 1
- 第 1 条类型为 Human（或 role=user），content 为 "check"
- 不包含 "SECRET_IN_MAIN"

### SI-02：excluded keys 必须不可见

当：调用 task(...)

则：

- 子 agent 回显的 state keys 不包含：
  - messages/todos/structured_response/skills_metadata/memory_contents

### SI-03：即使 schema 层标记为 private，仍必须在复制阶段过滤

给定：

- parent_state 中 `memory_contents` 很长

当：调用 task(...)

则：

- 子 agent 无法通过任何方式读到 memory_contents（回显/搜索均不包含）

### SI-04：child 不应继承主线程的 todo 上下文

给定：

- parent_state.todos 非空

当：调用 task(...)

则：

- 子 agent 无法在输入 state 中看到 todos
- 子 agent 若自行创建 todos，不应自动合并回主线程（见 state_merge 文档）

### SI-05：隔离不应破坏必要的全局配置

给定：

- parent_state 中包含 thread_id 或其它运行必需字段（由 Rust 实现定义）

当：调用 task(...)

则：

- 子 agent 能继续正常运行，不因缺少必要字段失败
- 说明隔离是“精确排除”，不是“清空全部 state”

## 4. 通过标准

- SI-01 ~ SI-05 全通过

