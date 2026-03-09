# Phase 4 详细迭代计划（SubAgentMiddleware：task 工具与子代理路由）

适用范围：本计划面向 [ITERATION_PLAN.md](ITERATION_PLAN.md#L158-L176) 的 Phase 4。目标是在 Rust 版 deepagents 的 Runtime/Tool/Middleware 体系上补齐“子代理（Subagents）”最小闭环：主 agent 通过 `task` 工具调用短生命周期子 agent，子 agent 的上下文与主线程严格隔离，并以受控方式把“最后结果 + 可合并 state”回传到主线程，且不允许子代理越权（root/命令权限）。

本计划的行为契约以现有验收文档为准：

- 验收基线：[subagents.md](../acceptance/subagents.md)
- Python 参考实现（行为参考，不依赖实现细节）：[subagents.py](../../../deepagents/libs/deepagents/deepagents/middleware/subagents.py)

本阶段强调三条原则：

- 契约先行：先把 `task` schema、隔离/合并规则、错误语义与嵌套策略固定下来，测试才可回归
- Trait-first：registry/路由/执行器均以 trait 抽象，默认实现仅作参考
- 安全默认：子代理不可绕过 root 边界与 execute 的审批/allow-list 语义

## 1. 完成定义（Definition of Done）

Phase 4 完成必须同时满足：

- `task` 工具可用：
  - LLM 可见 schema 仅包含 `description: string` 与 `subagent_type: string`
  - 可调用至少 2 个内置子代理（`general-purpose` 与 `echo-subagent`）
  - 能把子代理的“单条最终结果”回注主线程（tool message / tool result 语义稳定）
- 子代理 registry 与路由策略可用：
  - 支持注册、列出、按 `subagent_type` 解析
  - 默认路由行为稳定：未知类型、缺省类型、重复注册等均有明确策略
- state 隔离与受控合并落地：
  - parent → child：按排除列表过滤 state，且强制 child messages 仅包含任务描述
  - child → parent：只回传 child 的最后一条 assistant 输出；state_update 按排除列表过滤；合并策略可断言
- 安全性验收通过：
  - 子代理不能越过 root 读取/写入
  - 子代理不能绕过 execute 的审批/allow-list（与主线程同策略）
  - 通过 [subagents.md](../acceptance/subagents.md) 的 SA-01~SA-08（或在本文明确声明“拒绝嵌套”的固定行为，并有对应测试）
- 可维护性达标：
  - `SubAgentMiddleware`/registry/state 过滤与合并的单测覆盖关键分支
  - 对外公开 API 增量是可选、可扩展且不破坏已有用例（Phase 1.5/2/3）

## 1.1 范围与非目标（Scope / Non-goals）

范围（Phase 4 必做）：

- `task` 工具最小实现：可被 runtime 调用、可选择子代理、可回传结果与 state_update
- 子代理注册与路由：registry 抽象 + 默认 in-memory 实现 + 内置两个 subagent_type
- 状态隔离与受控合并：对齐 Python 的排除列表，行为可断言
- 权限边界继承：root 与 execute 审批/allow-list 不可被子代理绕过
- 测试：覆盖 SA-01~SA-08 的回归用例（可在本阶段选择“允许嵌套 + 深度限制”或“明确拒绝嵌套”）

非目标（Phase 4 不做，但需留好接口）：

- 子代理从磁盘加载（agents 目录发现、markdown frontmatter 等）：留到后续阶段
- 多类型 transport 兼容（PatchToolCallsMiddleware）：留到 Phase 5
- Skills 动态加载与校验：留到 Phase 6
- Memory/Summarization 的子代理持久化与共享策略：留到 Phase 7/8
- 并发执行/后台 subagent 会话（spawn/list/manage）：不在 Phase 4（与 `task` 的短生命周期模型不同）

## 2. 当前系统基线与缺口（Phase 4 启动时）

### 2.1 已有能力（可复用）

- Runtime/Provider 已具备最小闭环：
  - Runtime：`SimpleRuntime` 负责 provider 循环与 tool 执行（见 [simple.rs](../../crates/deepagents/src/runtime/simple.rs)）
  - Provider：`MockProvider` 可脚本化、确定性回归（见 [mock.rs](../../crates/deepagents/src/provider/mock.rs)）
- 工具执行入口与 middleware 链存在：
  - `DeepAgent::call_tool_stateful` 可执行工具并触发 middleware（见 [agent.rs](../../crates/deepagents/src/agent.rs#L55-L109)）
  - `FilesystemMiddleware` 已能回填 `AgentState.filesystem` 并产出 delta（见 [filesystem.rs](../../crates/deepagents/src/middleware/filesystem.rs)）
- execute 审批/审计入口已在 runtime 层接入（Phase 2），可被 Phase 4 复用（见 [simple.rs](../../crates/deepagents/src/runtime/simple.rs#L273-L430)）

### 2.2 Phase 4 必补缺口

以当前代码为 ground truth，本阶段的“从 0 到 1”能力已实现：

- `task` 工具拦截与短路：通过 `RuntimeMiddleware::handle_tool_call` 接管 `tool_name == "task"`，见 [SubAgentMiddleware](../../crates/deepagents/src/subagents/middleware.rs#L36-L135)
- 深度限制：`max_task_depth` 已实现（默认 2），见 [SubAgentMiddleware](../../crates/deepagents/src/subagents/middleware.rs#L16-L83)
- 状态模型：`AgentState` 已包含 `filesystem + extra`，可用于过滤/合并受控 keys，见 [state.rs](../../crates/deepagents/src/state.rs#L6-L13)

本阶段剩余工作主要集中在“对齐 Python 可观察语义 + 回归强度”：

- 与 Python `_EXCLUDED_STATE_KEYS` 的过滤/合并契约持续固化（并确保覆盖 messages/todos/skills_metadata/memory_contents/structured_response）。
- `tool_call_id/call_id` 的强约束语义对齐：Python `task()` 缺失 id 会直接失败；Rust 需要明确并通过测试锁死一致行为。
- tool result 形态与 messages 对齐策略：目前 `task` 返回 `{ content: final_text }`，但“只回传最后一条消息/以及如何绑定 call_id”需要作为契约固化并在 E2E 中回归。

## 3. 对外契约（必须冻结）

### 3.1 `task` 工具 schema（LLM 可见）

`task` 作为一个工具暴露给 provider/LLM，其输入 JSON 必须严格满足：

- `description: string`（必填，长度限制见 3.4）
- `subagent_type: string`（必填；允许空字符串但会按路由策略处理）

禁止把以下信息放入 schema（仅允许系统注入/运行期传递）：

- `tool_call_id/call_id`
- parent state/messages
- root、shell allow-list、审批/审计配置
- 子代理内部运行日志或 trace

### 3.2 子代理规范（SubAgentSpec）

本阶段只要求“内存注册 + 内置子代理”，但必须把规范固定为可扩展形态：

- `subagent_type: String`：路由主键（例如 `general-purpose`）
- `description: String`：用于工具说明/可观测性
- `runtime_profile: SubAgentRuntimeProfile`：
  - `provider: Arc<dyn Provider>`（子代理可用独立 provider；测试阶段通常用 MockProvider）
  - `skills: Vec<Arc<dyn SkillPlugin>>`（可空）
  - `runtime_config: RuntimeConfig`（max_steps/timeout）
- `agent_profile: SubAgentAgentProfile`：
  - `tools_policy: ToolsPolicy`（本阶段建议只支持 inherit/allow-list 两种）
  - `middleware_policy: MiddlewarePolicy`（本阶段建议强制 inherit 基础 middleware）
  - `backend_policy: BackendPolicy`（本阶段必须 inherit parent backend；不允许 child 指定不同 root）
- `merge_policy: StateMergePolicy`：state 的隔离/回传规则（见 3.3）
- `nesting_policy: NestingPolicy`：是否允许嵌套 task（见 3.5）

### 3.3 state 传递与合并（对齐 Python 行为）

排除列表必须与 Python `_EXCLUDED_STATE_KEYS` 对齐（验收基线见 [subagents.md](../acceptance/subagents.md#L28-L48)）：

- `messages`
- `todos`
- `structured_response`
- `skills_metadata`
- `memory_contents`

parent → child（准备 child 输入）：

- `child_state = clone(parent_state)`，但移除上述 keys
- 强制设置 `child_messages = [Human(description)]`
  - child 不继承 parent 的历史 messages（避免泄漏与污染）
  - child 不继承 “私有态”（skills/memory/todos/structured_response）

child → parent（回传）：

- 子代理执行完成必须能产生“至少 1 条 assistant 输出”（或等价的 `final_text`）
- parent 只回传 child 的最后一条 assistant 输出，包装为 tool message（绑定 tool_call_id/call_id）
- `state_update` 从 child_state 过滤上述 keys 后形成，并按 merge policy 合并到 parent_state

合并策略的最低要求（可断言）：

- 默认只允许合并 filesystem 与 `extra` 字段中非排除 keys
- filesystem 合并建议按 delta/reducer 进行（避免全量覆盖带来的语义不清），或保证“child_state 的 filesystem 起始于 parent 克隆”从而全量覆盖等价

### 3.4 输入预算与错误语义（稳定可回归）

为保证安全与可控性，需要固定以下边界：

- `description` 最大长度：建议 8KB（超出返回 `invalid_request`）
- `subagent_type` 最大长度：建议 128（超出返回 `invalid_request`）
- 未知 `subagent_type`：返回 `subagent_not_found`
- 子代理执行超时（provider timeout）：返回 `subagent_timeout`（并在 details 中携带 provider_timeout_ms）
- 子代理输出非法（缺少输出/缺少 messages）：返回 `subagent_invalid_output`
- task 嵌套策略违反：返回 `subagent_nesting_not_allowed` 或 `max_task_depth_exceeded`

### 3.5 嵌套 task（SA-08）策略选择

本阶段二选一并固化为契约（不得“有时支持有时崩溃”）：

- 选项 A（推荐）：支持嵌套，但限制最大深度 `max_task_depth = 2`（可配置），超出即拒绝
- 选项 B：明确拒绝嵌套 task（child 内调用 task 一律返回固定错误）

建议采用选项 A：实现成本可控且更贴近真实多代理编排需求，同时可通过深度限制避免递归失控。

## 3.6 对外 API / 兼容性变更清单（Phase 4 预期增量）

本阶段预期会引入以下“对外可见”的增量（应保持向后兼容、尽量 additive）：

- runtime / provider 协议增量：
  - `ProviderStep` 增加 `assistant_message`（或等价）以表达“非终止 assistant 输出”
  - `RunOutput`（如需要）补齐 child/parent 对“最后一条消息”的可回归表达（优先通过 tests 断言，不强制对外暴露更复杂结构）
- middleware 协议增量：
  - `Middleware` 新增可短路 hook（例如 `handle_tool_call`），用于接管 `task` 的执行路径
- state 模型增量：
  - `AgentState` 扩展 `extra`（用于承载可过滤键），并提供稳定的过滤/合并策略入口
- tools 列表增量：
  - runtime 的 tool specs 增加 `task`（或通过“系统工具列表”注入），保证 provider 可见

## 4. 核心设计与架构（Trait-first）

### 4.1 分层与职责

- Runtime：负责循环调度与 tool 执行（主线程与子线程共用）
- SubAgentMiddleware：负责拦截/处理 `tool_name == "task"` 的调用，并完成：
  - registry 路由
  - child 执行（创建 child runtime 并运行）
  - 结果回注主线程（tool result + messages/state 更新）
  - 安全边界继承（root/approval/audit）
- SubAgentRegistry：负责子代理的注册与解析
- StateTransfer/StateMerge：负责 state 过滤、隔离与合并策略（可测试、可替换）

### 4.2 `Middleware` 协议的最小扩展（让 task 可“接管执行”）

现有 middleware 只能 before/after，无法阻止底层工具执行。为实现 `task` 的“系统级工具”，需要增加一个“可短路”的 hook（保持向后兼容）：

- 新增 `handle_tool_call(...) -> Option<ToolResultOverride>`
  - 默认返回 `None`（不接管）
  - SubAgentMiddleware 在 `tool_name == "task"` 时返回 `Some(...)`，由 runtime 使用其结果而不再调用 `DeepAgent::call_tool_stateful`

该扩展应保持：

- additive：对现有 middleware 无破坏
- deterministic：多个 middleware 都想接管时，按 middleware 顺序第一命中即生效（或明确“只允许一个接管者”，否则报错）

### 4.3 子代理 registry（SubAgentRegistry）

建议 trait：

- `register(spec) -> Result<(), RegistryError>`
- `resolve(subagent_type) -> Option<CompiledSubAgent>`
- `list() -> Vec<SubAgentInfo>`

默认实现（Phase 4 目标）：

- `InMemorySubAgentRegistry`：HashMap 存储
- 预注册内置子代理：
  - `general-purpose`：用于真实任务（可用 MockProvider 驱动，后续替换成真实 provider）
  - `echo-subagent`：用于隔离断言（回显其看到的 messages/state keys）

### 4.4 子代理执行器（SubAgentExecutor）与继承策略

SubAgentExecutor 的核心输入：

- parent 运行期上下文：`backend/root/mode/approval/audit`
- parent messages/state（用于构造 child 输入与合并）
- tool_call_id/call_id（用于把 child 最终输出回注为 tool message）

继承约束（安全默认）：

- backend 必须继承 parent（同 root 边界）
- approval/audit 必须继承 parent（避免 child 绕过 execute 政策）
- tools/middleware：本阶段建议“基础能力强制继承 + 子代理可追加只读工具”，禁止 child 注入更高权限工具集合

### 4.5 统一状态模型（满足排除列表与过滤）

为对齐验收的 `_EXCLUDED_STATE_KEYS`，需要让 Rust 侧 state 能表达这些 keys 并可过滤。建议最小改造：

- 扩展 `AgentState`：
  - 保留强类型 `filesystem: FilesystemState`
  - 新增 `extra: BTreeMap<String, serde_json::Value>`（serde flatten 或显式字段均可）
    - 用于承载 `todos/structured_response/skills_metadata/memory_contents/messages` 等跨 middleware 状态
- 引入 `StateFilter`/`StateMerger`：
  - `filter_for_child(parent_state) -> child_state`
  - `merge_child_update(parent_state, child_state) -> ()`

备注：messages 是否应进入 `AgentState` 取决于后续整体架构；但 Phase 4 至少需要让“排除列表”可被结构化表达并可测试（否则无法验收 SA-03/SA-05）。

### 4.6 Runtime 的补齐点：可产生“多条 assistant 消息”

为满足“只回传 child 最后一条 message”的契约，需要 runtime 能表达：

- assistant message（非终止）与 final（终止）的区别

建议扩展 `ProviderStep`：

- 增加 `AssistantMessage { text }`：runtime 追加到 messages 并继续循环
- 保留 `FinalText { text }`：终止

这样可以用 MockProvider 精确构造 SA-04（多条 assistant → 只回传最后一条）的回归用例。

## 4.7 目录结构与模块边界建议

为降低耦合并便于后续 Phase 5/6/7 叠加，建议按以下边界拆分：

- `crates/deepagents/src/subagents/`
  - `protocol.rs`：`SubAgentSpec`/`CompiledSubAgent`/registry trait/state merge trait 等协议
  - `registry.rs`：`InMemorySubAgentRegistry` 默认实现
  - `middleware.rs`：`SubAgentMiddleware`（接管 `task`）
  - `state_merge.rs`：排除列表、filter/merge 默认实现
- `crates/deepagents/src/runtime/`
  - 仅维护“循环与编排”，避免把 subagents 的细节揉进 runtime

如果当前代码更偏“小文件扁平化”，也可先落一个 `subagents.rs`，但需保持内部分层清晰。

## 5. 详细迭代拆解（里程碑）

### M0：冻结契约与测试基线

- 输出
  - 本文档冻结 `task` schema、排除列表、错误码与嵌套策略
  - 将 [subagents.md](../acceptance/subagents.md) 的 SA-01~SA-08 映射为 Rust 侧可执行测试清单（见第 6 节）
- 验收
  - 可据此实现无歧义的测试（不依赖实现细节）

### M1：补齐 Middleware “可接管工具调用”能力

- 任务
  - 扩展 middleware 协议：新增可短路 hook（见 4.2）
  - runtime 执行工具时：
    - 先询问 middleware 是否接管
    - 未接管才走 `DeepAgent::call_tool_stateful`
  - 确保 tool_calls/tool_results 记录仍稳定（call_id 规则不变）
- 验收
  - 新增集成测试：插入一个“拦截某 tool 并返回固定结果”的 middleware，runtime 输出与预期一致

### M2：实现 SubAgentRegistry + 内置子代理

- 任务
  - 新增 `subagents` 模块（或放入 middleware/subagents.rs，按项目组织习惯）
  - `InMemorySubAgentRegistry` 实现注册/解析/列出
  - 提供内置子代理：
    - `general-purpose`：可用 MockProvider 驱动，输出固定文本用于闭环
    - `echo-subagent`：输出其看到的 messages 数量、首条内容、state keys（用于隔离断言）
- 验收
  - 单测：registry 的重复注册/未知类型/列出顺序与稳定性

### M3：实现 SubAgentMiddleware + `task` 最小闭环

- 任务
  - SubAgentMiddleware 在 `tool_name == "task"` 时接管执行：
    - 解析并严格校验 `description/subagent_type`
    - 通过 registry resolve 子代理
    - 构造 child 输入（state 过滤 + messages 重置）
    - 执行 child runtime（继承 backend/approval/audit）
    - 提取 child 最终输出（最后一条 assistant 或 final_text）
    - 将该输出作为 tool result 回注主线程，并做 state_update 合并
  - 明确嵌套策略与深度限制，行为固定（见 3.5）
- 验收
  - 通过 SA-01（最小闭环）与 SA-02/03（隔离）基础用例

### M4：StateTransfer/StateMerge 受控合并落地

- 任务
  - 落地 `_EXCLUDED_STATE_KEYS` 同款排除列表（常量 + 测试）
  - state 合并策略实现并可配置（默认策略满足验收）
  - 对 filesystem：
    - 推荐方案：在 child runtime 内累积 filesystem delta，回传 delta 到 parent 并 reducer 合并（更受控）
    - 备选方案：保证 child_state 基于 parent clone 并全量覆盖 filesystem（需证明等价且不丢字段）
- 验收
  - 通过 SA-05（state_update 过滤与合并）与 SA-07（子线程工具副作用存在，但不污染主 messages）

### M5：安全与审计：子代理不越权

- 任务
  - backend/root 强制继承：child 不能指定不同 root
  - execute 权限强制继承：child 走与 parent 一致的 approval/audit
  - 若启用 audit：记录子代理开始/结束边界与 tool_call_id（便于回溯）
- 验收
  - 安全测试：
    - root 越界读取被拒绝
    - execute 未在 allow-list 中被拒绝，且错误码可分类
    - audit JSONL 中可断言子代理边界事件（若本阶段纳入）

## 6. 测试计划（验收优先级最高）

测试以 [subagents.md](../acceptance/subagents.md) 的 SA-01~SA-08 为主线，建议落地为 Rust 集成测试（core 或 CLI 侧均可，但必须黑盒可回归）。

- SA-01：最小 task 闭环
  - MockProvider：主线程输出 tool_call(task)，子线程输出 final_text("HI")
  - 断言：主线程收到 tool result 内容为 "HI"
- SA-02：隔离：child messages 仅包含 description
  - echo-subagent 回显其收到的 messages
  - 断言：messages 长度为 1 且内容等于 description；不包含主线程敏感占位文本
- SA-03：隔离：excluded keys 不下发
  - parent_state.extra 塞入 `todos/skills_metadata/memory_contents/structured_response/messages`
  - echo-subagent 输出 state keys
  - 断言：排除 keys 不存在
- SA-04：回传：只回传最后一条 message
  - 子线程 provider 脚本输出多条 `AssistantMessage`，最后 `FinalText("final")`
  - 断言：主线程只看到 "final"
- SA-05：回传：state_update 过滤规则
  - child_state.extra 写入 allowed_key 与 todos/messages 等被排除字段
  - 断言：parent 合并后仅出现 allowed_key
- SA-06：子代理输出必须包含可回传内容
  - 构造一个 broken 子代理：不产生 assistant/final 输出
  - 断言：task 返回 `subagent_invalid_output`，主线程不崩溃
- SA-07：子线程可调用工具但不污染主线程历史
  - 子线程调用 write_file，然后 final_text("DONE")
  - 断言：文件存在；主线程 messages/tool_results 不包含子线程内部过程，仅包含 "DONE" 的 tool result
- SA-08：嵌套 task
  - 若采用“允许嵌套 + 深度限制”：构造 child 内再调用 task 的脚本并断言隔离链路正确
  - 若采用“拒绝嵌套”：断言固定错误码 `subagent_nesting_not_allowed`

## 7. 风险与取舍（提前声明，避免返工）

- 状态模型扩展风险：把更多字段放进 `AgentState` 可能触发序列化/兼容问题；建议通过 `extra` 承载，并对关键字段保留强类型
- Runtime 扩展风险：引入 `AssistantMessage` 会影响 provider/mock 脚本与 loop 行为；需要先锁定回归测试，再落地实现
- 安全边界风险：若允许子代理自定义工具集合/后端，容易产生越权路径；Phase 4 必须默认 inherit 并仅做最小可控扩展

## 8. 交付物清单（Deliverables）

- 文档
  - Phase 4 详细迭代计划（本文）
- 代码（实现阶段产出，应与本文一致）
  - `task` 工具最小实现（通过 SubAgentMiddleware 接管）
  - `SubAgentRegistry` + 默认 `InMemorySubAgentRegistry`
  - `SubAgentMiddleware`（隔离/合并/安全继承）
  - StateFilter/StateMerger（排除列表对齐 Python）
  - 测试套件：覆盖 SA-01~SA-08（含不越权与合并可控）

## 9. 待办事项（实现任务清单）

该清单用于实现落地时逐项勾选，避免遗漏关键契约点：

- 协议与类型
  - 新增/扩展 `ProviderStep` 表达非终止消息（用于 SA-04）
  - 扩展 `AgentState` 引入 `extra` 并确保 serde 行为稳定
  - 固化排除列表常量与过滤/合并默认策略
- Middleware 与 Runtime
  - 增加 middleware 可短路 hook，并在 runtime 执行工具前调用
  - 新增 `SubAgentMiddleware`：接管 tool_name=`task`
- Registry 与路由
  - 新增 `SubAgentRegistry` trait + `InMemorySubAgentRegistry`
  - 预注册 `general-purpose` 与 `echo-subagent`
  - 明确 unknown type、空 type、重复注册的策略与错误码
- 安全继承
  - 子代理强制 inherit backend/root
  - 子代理强制 inherit approval/audit，避免 execute 绕过
  - 明确嵌套策略（允许 + 深度限制，或拒绝）并加测试
- 测试落地
  - 为 SA-01~SA-08 建立集成测试文件与脚本（优先走 MockProvider）
  - 增加 1 个“越界读取/写入”负向用例与 1 个 “execute 未允许”负向用例

## 10. 验收检查表（Review Checklist）

- 契约对齐
  - `task` schema 仅包含 description/subagent_type
  - 排除列表与 [subagents.md](../acceptance/subagents.md) 一致
  - SA-08 的策略在实现与测试中是固定且可解释的
- 隔离与合并
  - child messages 仅包含 1 条 Human(description)
  - parent 仅接收 child 最后一条输出（无中间过程污染）
  - state_update 不包含排除 keys，合并策略可断言且无副作用
- 权限边界
  - root 越界被拒绝（读/写/grep/glob）
  - execute 不可绕过审批/allow-list（主/子一致）
- 可回归性
  - SA-01~SA-08 全部自动化，`cargo test` 稳定通过
  - 失败路径错误码稳定且可分类（便于上层展示/诊断）
