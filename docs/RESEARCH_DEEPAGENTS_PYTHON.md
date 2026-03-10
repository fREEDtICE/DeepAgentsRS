---
title: DeepAgents（Python 版）代码调研与 Rust 迁移方案
source_repo_root: ../../deepagents/
generated_at: 2026-03-05
---

## 0. 范围与目标

本调研以 `../../deepagents/` 为唯一事实来源，目标是：

- 还原 deepagents（Python）端到端运行链路与关键语义边界（agent loop / middleware / backend / state）。
- 提炼出 Rust 版本需要对齐的“可观察行为”（tool schema、state key、隔离策略、落盘路径、错误语义等）。
- 形成可落地的 Rust 架构方案（分层、trait、模块拆分与分阶段交付路径）。

说明：本仓库同时包含 SDK（`libs/deepagents`）与 CLI（`libs/cli`）；本调研以 SDK 语义为主，CLI 只用于验证运行循环与 HITL/compact 等能力的实际使用方式。

## 1. 结论摘要（先给决策结论）

- deepagents 的核心不是“复杂 LangGraph 子图”，而是：**工具调用驱动的 agent loop + 可组合的 middleware 链 + 可替换的 backend 协议**。`create_deep_agent` 主要做默认能力栈组装，然后委托给 LangChain 的 `create_agent` 执行（见 [graph.py:L107-L316](../../deepagents/libs/deepagents/deepagents/graph.py#L107-L316)）。
- 所有“默认能力”（filesystem、subagent task、summarization、todo、HITL 等）都通过 middleware 完成两类工作：
  - 每次模型调用前：动态注入 system prompt 片段与可用 tools；
  - 每次 tool call 前/后：拦截执行、改写结果、更新 state。
  入口在 [middleware/__init__.py](../../deepagents/libs/deepagents/deepagents/middleware/__init__.py)。
- 运行环境差异（真实磁盘/内存态/沙箱执行/路由/持久 store）下沉到 backend：`BackendProtocol`/`SandboxBackendProtocol` + `CompositeBackend`（见 [protocol.py](../../deepagents/libs/deepagents/deepagents/backends/protocol.py)、[composite.py](../../deepagents/libs/deepagents/deepagents/backends/composite.py)）。

## 2. Python 端总体结构（模块与职责）

### 2.1 SDK 主入口：`create_deep_agent`

`create_deep_agent` 的工作分为三步：

1) 选择/解析模型（默认 Claude Sonnet 4.6；`openai:` 默认走 Responses API；见 [graph.py:L70-L105](../../deepagents/libs/deepagents/deepagents/graph.py#L70-L105)）。

2) 构建一个“通用 subagent（general-purpose）”，并强制装配默认 middleware 栈（Todo + Filesystem + Summarization + PromptCaching + PatchToolCalls；可选 Skills/HITL），见 [graph.py:L214-L233](../../deepagents/libs/deepagents/deepagents/graph.py#L214-L233)。

3) 构建主 agent 的 middleware 栈（Todo → Memory? → Skills? → Filesystem → SubAgent(task) → Summarization → PromptCaching → PatchToolCalls → 用户 middleware → HITL?），见 [graph.py:L269-L294](../../deepagents/libs/deepagents/deepagents/graph.py#L269-L294)。

最终调用 LangChain 的 `create_agent(...)` 并设置 recursion limit 1000，见 [graph.py:L304-L316](../../deepagents/libs/deepagents/deepagents/graph.py#L304-L316)。

### 2.2 基础 System Prompt

基础 prompt 内容在 SDK 内置字符串 `BASE_AGENT_PROMPT`（与 [base_prompt.md](../../deepagents/libs/deepagents/deepagents/base_prompt.md) 相同意图），并通过 middleware 注入各 tool 的使用说明。快照可参考 smoke test： [system_prompt_without_execute.md](../../deepagents/libs/deepagents/tests/unit_tests/smoke_tests/snapshots/system_prompt_without_execute.md)。

Rust 迁移要点：不要只复刻“文案”，更要复刻“文案所约束的可观察行为”（比如 write_todos 并行调用必须被拒绝，task 生命周期与隔离策略等）。

## 3. Middleware 体系（deepagents 的真正核心）

### 3.1 Middleware 的两类拦截点

从实际实现看，deepagents 的 middleware 需要覆盖这些拦截点：

- 模型调用前（wrap_model_call）：注入 system prompt 片段、动态过滤/添加 tool；
- 工具调用前/后（wrap_tool_call）：拦截参数、拦截结果、把“过大结果”落盘、补齐悬挂 tool call；
- 运行开始前（before_agent）：一次性加载 memory/skills 等并写入 state。

Rust 端建议将其抽象为固定接口（类似 request/response middleware），以保证“可插拔”和“顺序可控”。

#### 默认 middleware 栈顺序也是语义的一部分

`create_deep_agent` 不只是“把能力都挂上去”，还会按固定顺序装配 middleware。由于 middleware 会在不同拦截点改写「模型可见 messages」「system prompt」「可用 tools」以及「tool result」，因此顺序会改变可观察行为（例如：先做 Summarization 再做 PatchToolCalls 与反过来效果不同；Filesystem 的“大结果落盘”如果发生在 Summarization 之前/之后也会影响上下文大小）。

主 agent 与通用 subagent 的默认栈请以源码为准：见 [graph.py:L214-L316](../../deepagents/libs/deepagents/deepagents/graph.py#L214-L316)。

### 3.2 FilesystemMiddleware：文件工具 + 安全校验 + 大结果落盘

实现文件： [filesystem.py](../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py)。

#### 工具清单与关键语义

- `ls(path)`：目录列表；强制 `validate_path`；返回 path 列表并截断（见 [filesystem.py:L479-L518](../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L479-L518)）。
- `read_file(file_path, offset=0, limit=100)`：
  - 文本：分页读取、超阈值截断并在尾部提示；
  - 图片：返回多模态 ToolMessage（支持 png/jpg/jpeg/gif/webp），并在 additional_kwargs 写入路径/类型（见 [filesystem.py:L520-L628](../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L520-L628)）。
- `write_file(file_path, content)` / `edit_file(file_path, old_string, new_string, replace_all=false)`：
  - 若 backend 返回 `files_update`，则用 `Command(update=...)` 回写 state（对齐 StateBackend 语义）；
  - 否则返回字符串确认（见 [filesystem.py:L630-L771](../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L630-L771)）。
- `glob(pattern, path="/")`：有 20s 超时；返回 path 列表并截断（见 [filesystem.py:L772-L825](../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L772-L825)）。
- `grep(pattern, path=None, glob=None, output_mode="files_with_matches")`：
  - 调用 backend 的结构化 grep，格式化输出并截断（见 [filesystem.py:L826-L872](../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L826-L872)）；
  - 注意：`grep_raw` 是字面量 substring 匹配，不是正则（这会影响用户预期与示例写法），见 [filesystem.py:L865-L910](../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L865-L910)。
- `execute(command, timeout=None)`：仅在 backend 支持 sandbox execute 时暴露；timeout 必须在范围内；并存在“后端是否接受 timeout 参数”的兼容判断（见 [filesystem.py:L873-L991](../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L873-L991)、[protocol.py:L495-L515](../../deepagents/libs/deepagents/deepagents/backends/protocol.py#L495-L515)）。

#### execute 的错误语义与兼容分支（可观察行为）

`execute` 工具的失败通常不会抛出异常给模型侧，而是返回可读的错误字符串（这属于可观察行为，需要 Rust 对齐）：

- backend 不支持 execute：返回固定前缀的错误文本（而非异常）；
- backend 不接受 `timeout` 参数：通过 `execute_accepts_timeout()` 做签名探测与缓存，不支持时返回明确错误文本提示升级/移除 timeout；
- 仅部分异常会被捕获并字符串化（`NotImplementedError`/`ValueError`），其余异常可能上抛到框架层并导致整轮失败。

证据见 [filesystem.py:L912-L1024](../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L912-L1024) 与 [protocol.py:L437-L515](../../deepagents/libs/deepagents/deepagents/backends/protocol.py#L437-L515)。

#### 路径安全：validate_path

校验在 backend utils： [utils.py:L234-L297](../../deepagents/libs/deepagents/deepagents/backends/utils.py#L234-L297)：

- 禁止任何 path segment 为 `..`；
- 禁止 `~` 开头；
- 禁止 Windows 盘符绝对路径（`^[a-zA-Z]:`）；
- `normpath` 后强制以 `/` 开头，并做二次 `..` 防御；
- 预留 `allowed_prefixes` 白名单能力（middleware 当前未启用，但 Rust 建议保留并默认可配置）。

注意：当前 Python `grep` 的 path 参数未走 `validate_path`，Rust 迁移时需要决策是“兼容现状”还是“默认更安全”。

#### filesystem state 合并的“删除语义”

FilesystemMiddleware 维护的 `filesystem.files` 是一个“可合并”的 state 子树。其 reducer 约定：如果右侧 patch 中某个 path 的值为 `None`，表示删除该文件条目（dict pop），而不是把值设为 null。这会影响 checkpoint 合并/回放与“文件视图”的一致性，应视为必须对齐的语义。

证据见 [filesystem.py:L90-L133](../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L90-L133)。

#### 大工具结果落盘（Large Tool Result Offload）

FilesystemMiddleware 会在 tool 调用后拦截返回值，如果近似 token（按 4 chars/token）超过阈值（默认 20000 token），会把结果写到：

`/large_tool_results/{sanitize_tool_call_id(tool_call_id)}`

并把 ToolMessage 替换为“文件引用 + 头尾预览 + 引导使用 read_file 分页读取”的模板（见 [filesystem.py:L1089-L1165](../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L1089-L1165)）。

Rust 迁移要点：这不是“可选优化”，而是避免上下文爆炸的关键机制（同时它也依赖 CompositeBackend 路由把这些虚拟文件写到合适的位置）。

### 3.3 SubAgentMiddleware：`task` 工具与严格隔离

实现文件： [subagents.py](../../deepagents/libs/deepagents/deepagents/middleware/subagents.py)。

#### `task` 工具 schema

暴露给模型的参数只有两个：

- `description: str`
- `subagent_type: str`

runtime（state/tool_call_id 等）由框架注入，不进入 schema（见 [subagents.py:L430-L471](../../deepagents/libs/deepagents/deepagents/middleware/subagents.py#L430-L471)）。

#### tool_call_id 是硬约束（没有就直接失败）

`task()` 的实现要求运行时必须提供 `tool_call_id`；如果缺失会直接 `ValueError`，而不是返回“错误字符串”给模型继续跑。这意味着 Rust runner 若要对齐行为，需要保证 tool call 协议完整（每个 tool call 都有稳定 id，且会以 ToolMessage 回填对齐）。

证据见 [subagents.py:L438-L446](../../deepagents/libs/deepagents/deepagents/middleware/subagents.py#L438-L446)。

#### parent → subagent：只传任务描述，不继承长对话

子代理输入 state 的构造规则（必须对齐）：

- 复制父 state，但过滤 `_EXCLUDED_STATE_KEYS={"messages","todos","structured_response","skills_metadata","memory_contents"}`（见 [subagents.py:L115-L128](../../deepagents/libs/deepagents/deepagents/middleware/subagents.py#L115-L128)）。
- 强制设置 `subagent_state["messages"] = [HumanMessage(description)]`（见 [subagents.py:L422-L428](../../deepagents/libs/deepagents/deepagents/middleware/subagents.py#L422-L428)）。

这保证 subagent 线程上下文隔离，避免主线程消息与私有内容泄漏到子线程。

#### subagent → parent：只回传最后一条消息

工具返回的是 `Command(update=...)`，其中：

- state_update 过滤同一套 excluded keys；
- `messages` 只回传最后一条 message 的内容，包装成 ToolMessage(tool_call_id=...)（见 [subagents.py:L412-L420](../../deepagents/libs/deepagents/deepagents/middleware/subagents.py#L412-L420)）。

Rust 迁移要点：这是控制 token 与控制信息泄漏的关键约束；不要“把子代理全消息合并回主线程”。

### 3.4 SummarizationMiddleware：用 `_summarization_event` 改写“模型看到的 messages”

实现文件： [summarization.py](../../deepagents/libs/deepagents/deepagents/middleware/summarization.py)。

核心设计：**不直接修改 state.messages**，而是写一个私有事件 `_summarization_event`，在模型调用前把 state.messages 映射成 effective messages。

#### 状态键与事件结构

`SummarizationEvent = {cutoff_index, summary_message, file_path}`（见 [summarization.py:L99-L111](../../deepagents/libs/deepagents/deepagents/middleware/summarization.py#L99-L111)）。

#### effective messages 重建

- 若无事件：effective == state.messages
- 若有事件：effective == `[summary_message] + state.messages[cutoff_index..]`（见 [summarization.py:L477-L514](../../deepagents/libs/deepagents/deepagents/middleware/summarization.py#L477-L514)）

#### 链式压缩 cutoff 折算

当已经压缩过一次，effective 的 index=0 是 summary_message（它不是 state 里的真实消息），因此需要把 effective cutoff 翻译回 state cutoff：

`state_cutoff = prior_cutoff + effective_cutoff - 1`

（见 [summarization.py:L516-L542](../../deepagents/libs/deepagents/deepagents/middleware/summarization.py#L516-L542)）。

#### 历史落盘路径与格式

默认写入 `/conversation_history/{thread_id}.md`，每次追加 markdown section（见 [summarization.py:L361-L395](../../deepagents/libs/deepagents/deepagents/middleware/summarization.py#L361-L395)、[summarization.py:L712-L784](../../deepagents/libs/deepagents/deepagents/middleware/summarization.py#L712-L784)）。

Rust 迁移要点：

- 必须保留 `_summarization_event` 机制；否则“自动压缩 / 手动 compact / checkpoint state 复用”会割裂。
- 落盘失败不应阻断压缩（file_path 可以为 None）。

#### Summarization 还会裁剪旧 tool args（避免上下文污染）

Summarization 不仅会把历史消息摘要并落盘，还会对被“驱逐到摘要之前”的旧 `AIMessage.tool_calls` 做额外清理：默认会截断旧的 `write_file/edit_file` 参数（尤其是大段 content/patch），避免在后续轮次里持续占用上下文。

证据见 [summarization.py:L680-L710](../../deepagents/libs/deepagents/deepagents/middleware/summarization.py#L680-L710)。

#### 手动工具：`compact_conversation`

同一文件里还有 `SummarizationToolMiddleware` 提供 `compact_conversation` 工具，它会生成 summary 并更新 `_summarization_event`，同时返回一条 ToolMessage 确认（见 [summarization.py:L1166-L1240](../../deepagents/libs/deepagents/deepagents/middleware/summarization.py#L1166-L1240)）。

CLI 里也存在“直接读写 checkpoint state 实现 compact”的路径（见 [app.py:L1503-L1717](../../deepagents/libs/cli/deepagents_cli/app.py#L1503-L1717)），这验证了 event 机制是对外可依赖的稳定抽象。

### 3.5 PatchToolCallsMiddleware：补齐悬挂 tool call

目的：防止历史里出现“AI 发了 tool_call 但缺对应 ToolMessage”的悬挂情况；开跑前补一个“已取消” ToolMessage 以保持对齐（见 [patch_tool_calls.py](../../deepagents/libs/deepagents/deepagents/middleware/patch_tool_calls.py)）。

Rust 迁移要点：如果 Rust runner 也支持“恢复历史继续跑”，必须有同类修复，否则 tool_call 对齐会被破坏（尤其会影响 UI 渲染与后续轮次）。

### 3.6 TodoListMiddleware：write_todos 的并行调用防线

TodoListMiddleware 的源码不在本仓库（来自 langchain 依赖），但本仓库测试明确了关键规则：

- 同一轮 assistant 输出里若并行多个 `write_todos` tool calls，必须全部拒绝且不更新 state.todos（见 [test_todo_middleware.py](../../deepagents/libs/deepagents/tests/unit_tests/test_todo_middleware.py)）。

Rust 迁移要点：这条必须落到“执行层强约束”，不能只依赖 system prompt 文案。

### 3.7 MemoryMiddleware：私有 state 与失败语义

实现文件： [memory.py](../../deepagents/libs/deepagents/deepagents/middleware/memory.py)。

MemoryMiddleware 在 `before_agent` 阶段加载 memory 内容，并写入私有 state 键 `memory_contents`（通过 `PrivateStateAttr` 标注，意味着它不应作为“对外可见/可持久化 state”随意传播给子代理或外部）。

关键可观察语义：

- 加载失败的处理并非统一“返回错误字符串”：除“文件不存在”可被忽略外，其它失败路径会直接抛出异常中断本次运行（例如 download 失败），这会影响 Rust 版本的错误/恢复策略。

证据见 [memory.py:L70-L270](../../deepagents/libs/deepagents/deepagents/middleware/memory.py#L70-L270)。

## 4. Backend 协议（环境隔离与能力注入点）

实现文件： [protocol.py](../../deepagents/libs/deepagents/deepagents/backends/protocol.py)。

### 4.1 核心数据结构

- `FileInfo{path,is_dir?,size?,modified_at?}`（见 [protocol.py:L93-L104](../../deepagents/libs/deepagents/deepagents/backends/protocol.py#L93-L104)）
- `GrepMatch{path,line,text}`（见 [protocol.py:L106-L112](../../deepagents/libs/deepagents/deepagents/backends/protocol.py#L106-L112)）
- `WriteResult{error?,path?,files_update?}` / `EditResult{error?,path?,files_update?,occurrences?}`（见 [protocol.py:L114-L164](../../deepagents/libs/deepagents/deepagents/backends/protocol.py#L114-L164)）
- `ExecuteResponse{output,exit_code?,truncated}`（见 [protocol.py:L420-L493](../../deepagents/libs/deepagents/deepagents/backends/protocol.py#L420-L493)）

### 4.2 协议分层

- `BackendProtocol`：文件/grep/glob/upload/download 等能力；
- `SandboxBackendProtocol`：在 BackendProtocol 上扩展 `execute/aexecute`。

FilesystemMiddleware 会在模型调用前动态决定是否暴露 execute（并基于 `CompositeBackend.default` 判断），见 [filesystem.py:L273-L290](../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L273-L290)。

### 4.3 CompositeBackend：虚拟路径到多后端路由

实现： [composite.py](../../deepagents/libs/deepagents/deepagents/backends/composite.py)。

- 最长前缀匹配路由；
- `"/memories"`（无尾斜杠）路由时仍应落到该 backend，并把传入子 backend 的路径映射为 `"/"`；
- `"/memories/..."` 会剥离前缀并保证传入子 backend 的路径以 `/` 开头。

Rust 迁移要点：offload 文件（`/large_tool_results/...`、`/conversation_history/...`）依赖路由把“虚拟文件”落到合适位置，CompositeBackend 是稳定抽象。

## 5. 子系统：CLI 的运行循环（用于验证 runner 形态）

虽然 Rust 版先不需要 TUI，但 CLI 代码提供了 Python 端真实运行形态的证据：

- 非交互模式：消费 `agent.astream(stream_mode=["messages","updates"], subgraphs=True)`；捕获 interrupt 后 `Command(resume=...)` 续跑（见 [non_interactive.py:L488-L564](../../deepagents/libs/cli/deepagents_cli/non_interactive.py#L488-L564)）。
- Textual UI：同样消费 messages/updates 流，但做 UI 拼接（见 [textual_adapter.py](../../deepagents/libs/cli/deepagents_cli/textual_adapter.py)）。

Rust 迁移要点：需要一个可流式产出“messages + state updates + interrupt”事件的 runner 接口，CLI/UI 才能复刻体验。

## 6. Rust 版本方案（按“语义对齐”而非“逐行翻译”）

### 6.1 目标：先做可运行内核，再扩展生态

建议两层产品目标：

- Core：对齐 SDK 语义（tools/middleware/backend/state/interrupt/summarization offload）。
- Extras：CLI/TUI、技能生态、provider 特性（prompt caching）、tracing 等。

### 6.2 Rust 核心抽象（建议）

#### State / Message / Command（等价语义）

- `State = serde_json::Map<String, Value>`（开放式，便于 middleware 与 tool patch）
- `Command { update: Option<StatePatch>, resume: Option<Value> }`
- `Message` 支持 `ToolMessage{tool_call_id,...}` 与 `AssistantMessage{tool_calls:[{id,name,args}]}`。

#### Tool

- `trait Tool { fn name(&self)->&str; fn schema(&self)->Value; async fn call(&self, args: Value, rt: &mut ToolRuntime) -> ToolReturn }`
- `ToolRuntime` 注入：`state`、`tool_call_id`、配置、backend 引用等（LLM schema 不可见）。

#### Middleware

建议统一为四个拦截点（对应 Python wrap/before）：

- `before_run(state, config) -> Option<StatePatch>`
- `transform_model_request(req, state, config) -> req`
- `transform_tool_result(tool_name, tool_call_id, result, state, config) -> (result, Option<StatePatch>)`
- `after_run(state, config) -> Option<StatePatch>`

#### Runner（循环式 agent loop）

实现一个不依赖“图引擎”的 runner：

1) 组装模型请求（messages + system prompt + tools）
2) 调模型生成 assistant message
3) 解析 tool_calls 并逐个执行（支持 interrupt）
4) 合并 Command.update 到 state，追加 ToolMessage
5) 直到无 tool_calls 或触发终止条件

这能最直接复刻 Python `create_agent + middleware` 的可观察行为。

### 6.3 必须对齐的行为清单（Rust 验收用）

- Filesystem：
  - `read_file(offset/limit 默认 0/100)` 分页语义；
  - `validate_path` 规则；
  - grep 为字面量匹配（非 regex），并保留 output_mode 格式化；
  - `execute` 仅在 sandbox backend 暴露；
  - `execute` 的错误语义（不支持执行/timeout 参数兼容/错误文本）应可观察地对齐；
  - large tool result offload 写入 `/large_tool_results/...` 并返回引用模板。
  - filesystem reducer 对 `None` 代表删除的合并语义需对齐。
- Subagents：
  - `_EXCLUDED_STATE_KEYS` 过滤；
  - child messages 强制为 `[Human(description)]`；
  - 只回传 child 最后一条消息；
  - tool result 必须带 tool_call_id 对齐。
  - `tool_call_id` 缺失时的失败语义需要对齐（Python 直接抛错）。
- Summarization：
  - `_summarization_event` 事件机制；
  - effective messages 计算与 cutoff 折算；
  - 落盘 `/conversation_history/{thread_id}.md`，失败不阻断；
  - 手动 `compact_conversation` 更新 event。
  - 摘要时对旧 `write_file/edit_file` tool args 的裁剪语义需对齐。
- Todo：
  - 同轮多次 `write_todos` 必须拒绝并不更新 todos。
- PatchToolCalls：
  - 修复悬挂 tool_call 的历史一致性。
- HITL：
  - 指定 tool 上可 interrupt，并可通过 `Command{resume:...}` 继续。
- Memory：
  - `memory_contents` 私有 state 的传播边界；
  - memory 加载失败的“抛异常中断”语义与可恢复策略需要明确并对齐（至少行为要稳定）。

## 7. Rust 工程拆分建议（crate 结构）

- `deepagents-core`：Message/State/Command/Event、Runner、tool/middleware trait、JSON schema 生成
- `deepagents-backends`：Backend trait + FilesystemBackend/StateBackend/CompositeBackend/SandboxBackend
- `deepagents-middleware`：Filesystem/Subagents/Summarization/Todo/PatchToolCalls/HITL/Memory/Skills
- `deepagents-models`：provider 适配（可选拆出）
- `deepagents-cli`：先非交互，再 TUI（后置）

## 8. 参考入口（建议从这些文件开始移植）

- Agent 组装策略： [graph.py](../../deepagents/libs/deepagents/deepagents/graph.py)
- Filesystem tools 与 offload： [filesystem.py](../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py)
- Subagent task 与隔离： [subagents.py](../../deepagents/libs/deepagents/deepagents/middleware/subagents.py)
- Summarization event 与落盘： [summarization.py](../../deepagents/libs/deepagents/deepagents/middleware/summarization.py)
- Backend 协议： [protocol.py](../../deepagents/libs/deepagents/deepagents/backends/protocol.py)
- CompositeBackend 路由： [composite.py](../../deepagents/libs/deepagents/deepagents/backends/composite.py)
- validate_path 实现： [utils.py](../../deepagents/libs/deepagents/deepagents/backends/utils.py#L234-L297)
- Todo 并行防线测试： [test_todo_middleware.py](../../deepagents/libs/deepagents/tests/unit_tests/test_todo_middleware.py)

## 9. 附录：Rust 现状差距速览（风险提示）

本节不作为 Python 行为事实来源，仅用于把“迁移验收清单”与当前 DeepAgentsRS 代码现状对齐，避免在计划/排期上产生误判。

- Runner 事件流：Python CLI 依赖 `messages + updates + interrupt/resume` 的流式运行；当前 Rust `SimpleRuntime` 是一次性 `RunOutput`，尚无 interrupt/resume 语义。
- Middleware 能力面：Python middleware 覆盖 `before_agent + wrap_model_call + wrap_tool_call`；当前 Rust middleware 只有 tool 前后钩子，无法承载 summarization / patch_tool_calls / 动态 tools 注入等核心机制。
- 路径模型：Python 是虚拟路径（`/` 开头）+ `validate_path` + `CompositeBackend`；当前 Rust 偏向 OS path + root 边界约束，容易与 offload/历史落盘路径约定产生偏差。
- execute 门禁：Rust 侧通常存在“执行前硬拦截 + 运行时审计/审批”的双层门禁（需要在最终对齐设计里明确）。
- 服务化入口：DeepAgentsRS 额外提供 ACP server（会话化 call_tool + state_version），属于 Rust 侧新增集成形态，和 Python SDK/CLI 的 loop 不是同一层抽象。
