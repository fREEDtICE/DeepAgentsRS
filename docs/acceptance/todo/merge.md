---
title: Core Todo E2E - merge/replace 语义
scope: core
---

## 1. 端到端效果

write_todos 的核心是把“结构化 todo 列表”写回 state.todos。端到端必须明确并固定两种语义：

- merge=false：完全替换（replace）
- merge=true：按 id 合并（partial update）

## 2. 验收环境

- ScriptedModel 驱动 tool_calls
- 初始 state.todos 可控
- artifacts 包含 final_state.json 与 events.jsonl

## 3. E2E 场景（必测）

### TM-01：merge=false 完全替换

给定：

- 初始 todos=[a,b]

当：write_todos(merge=false, todos=[c])

则：

- final_state.todos 只有 [c]

### TM-02：merge=true 按 id 合并（更新部分字段）

给定：

- 初始 todos=[{id:"a",content:"A",status:"pending",priority:"high"}]

当：write_todos(merge=true, todos=[{id:"a",status:"completed"}])

则：

- id="a" 仍存在
- status 变为 completed
- content/priority 保持不变

### TM-03：merge=true 新 id 追加

给定：

- 初始 todos=[a]

当：write_todos(merge=true, todos=[b])

则：

- final_state.todos 包含 a 与 b
- 顺序策略固定（建议按原顺序 + 新增追加）

### TM-04：重复 id 输入的错误语义

给定：

- write_todos(merge=false, todos=[{id:"a"...},{id:"a"...}])

当：执行

则：

- 返回明确错误
- state.todos 不变化

### TM-05：summary 门禁（完成态转移才允许）

给定：

- 初始 todos=[{id:"a",status:"pending"}]

当：

- Case A：write_todos(merge=true, todos=[{id:"a",status:"completed"}], summary="done")
- Case B：write_todos(merge=true, todos=[{id:"a",status:"pending"}], summary="x")

则：

- Case A：成功，summary 被接受或记录（具体落点固定）
- Case B：失败或忽略 summary（两者择一，但必须固定并可诊断）

## 4. 通过标准

- TM-01 ~ TM-05 全通过

