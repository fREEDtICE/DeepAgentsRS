---
title: Extras CLI E2E - 非交互模式（脚本化）
scope: extras
---

## 1. 端到端效果

非交互 CLI 的端到端效果是：在纯命令行环境（CI）下，以确定性输入驱动 agent 完成任务，并产出可判定的产物：

- stdout/stderr 输出格式稳定（可快照对比）
- 任务执行过程可导出为 events.jsonl（或等价）
- 文件副作用在受控工作区可断言
- 支持 HITL 的情况下可通过“预置批准策略/响应文件”完成端到端闭环

## 2. 验收环境

- 使用 ScriptedModel（确定性），不依赖真实 provider
- 工作区使用临时目录（workspace tempdir）
- 固定 thread_id（例如 `e2e_thread`）以便断言 conversation_history 路径
- CLI 必须支持以下能力之一（两者择一但要固定）：
  - `--artifacts-dir <dir>` 指定产物输出目录
  - 或默认在 workspace 下创建固定目录（例如 `.deepagents/`）

## 3. E2E 场景（必测）

### CN-01：最小任务（无工具）

给定：

- CLI 输入：一条用户消息 "hello"
- ScriptedModel 返回 Assistant("world")

当：运行 CLI

则：

- stdout 包含 "world"
- exit code == 0
- artifacts 中包含 final_state.json/events.jsonl

### CN-02：文件任务（write/read）

给定：

- ScriptedModel 依次调用：
  - write_file("/a.txt","x")
  - read_file("/a.txt",0,10)
  - Assistant("done")

当：运行 CLI

则：

- workspace 中存在 a.txt，内容为 "x"
- stdout 中出现 read_file 的结果或摘要（取决于渲染策略，但必须可判定）

### CN-03：大结果 offload 的 CLI 可用性

给定：

- 触发 `/large_tool_results/...` 的 offload（阈值调小）

当：运行 CLI

则：

- stdout 中出现引用路径 `/large_tool_results/...`
- workspace/large_results 目录快照中存在对应文件
- 允许通过后续 CLI 命令或同一会话 read_file 分页读取该引用文件

### CN-04：summarization 落盘可观察

给定：

- 触发 summarization（阈值调小）

当：运行 CLI

则：

- artifacts 中出现 `/conversation_history/e2e_thread.md`（或映射到 workspace 下的等价路径）
- stdout 中出现 summary 提示（包含该虚拟路径或等价引用）

### CN-05：HITL 自动批准策略（可选但建议）

给定：

- interrupt_on={"edit_file":true}
- CLI 以参数指定自动批准策略：
  - 方案 A：`--auto-approve`（全部 approve）
  - 方案 B：`--approval-file <json>`（对每个 interrupt 给出 approve/reject/edit）

当：运行 CLI 执行 edit_file

则：

- 不需要交互输入也能完成端到端闭环
- 文件副作用符合策略
- artifacts 记录每次 interrupt 的决策（可选但推荐）

### CN-06：退出码与错误传播

给定：

- ScriptedModel 触发不可恢复错误（例如 tool_call_id 缺失）

当：运行 CLI

则：

- exit code != 0
- stderr（或 stdout）包含可诊断错误
- artifacts 仍落盘（至少包含 events 与错误摘要）

## 4. 通过标准

- CN-01 ~ CN-06 全通过
- CI 可在无网络/无真实 API key 条件下稳定运行

