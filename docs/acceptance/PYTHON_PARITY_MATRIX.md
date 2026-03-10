# Rust 对齐 Python 验收表（Parity Matrix）

本文件以 Python 版 `create_deep_agent()` 的默认栈为基准，对齐 Rust 版当前“可运行默认路径”（`deepagents-cli run` 注入的 `SimpleRuntime.runtime_middlewares`），逐项映射工具、schema、状态结构、中间件顺序、默认安全策略与 CLI 行为。

- Python 代码基准：`deepagents/libs/deepagents/deepagents/graph.py:create_deep_agent`：[graph.py:L210-L316](../../../deepagents/libs/deepagents/deepagents/graph.py#L210-L316)
- Rust 运行基准：`deepagents-cli/src/main.rs` 中组装 `runtime_middlewares` 并注入 runtime：[main.rs:L444-L514](../../crates/deepagents-cli/src/main.rs#L444-L514)

## 0) 口径说明

- Rust `SimpleRuntime` 本体默认 `runtime_middlewares` 为空，是否“默认挂载”取决于上层（CLI/ACP/集成方）如何注入。本文采用 CLI 的默认注入顺序作为“产品默认路径”。
- Python 默认包含 `TodoListMiddleware` 与 `AnthropicPromptCachingMiddleware` 等 LangChain/LangGraph 生态能力；Rust 当前无等价实现，按“缺失”或“需自研/接入”记录。

## 1) 中间件顺序对齐（Main Agent）

### 1.1 Python 默认顺序（主 agent）

`TodoList` →（可选）`Memory` →（可选）`Skills` → `Filesystem` → `Subagents` → `Summarization` → `AnthropicPromptCaching` → `PatchToolCalls` →（可选）用户 middleware →（可选）`HITL`

- 代码：[graph.py:L269-L294](../../../deepagents/libs/deepagents/deepagents/graph.py#L269-L294)

### 1.2 Rust CLI 默认顺序（run）

`TodoList` →（默认启用）`Memory` →（可选）`Skills` → `FilesystemRuntime` → `Subagents` →（默认启用）`Summarization` → `PromptCaching` → `PatchToolCalls`

- 代码：[main.rs:L444-L524](../../crates/deepagents-cli/src/main.rs#L444-L524)

### 1.3 中间件映射表

| 能力 | Python 默认位置 | Rust 默认位置（CLI run） | 对齐度 | 关键差异 / 备注 |
|---|---|---|---|---|
| PatchToolCalls | 靠后（接近末尾） | 靠后（Summarization/PromptCaching 之后） | 基本对齐 | Rust 侧还会补齐 provider tool-call 的 call_id 与 arguments 形态：[patch_tool_calls.rs:L86-L156](../../crates/deepagents/src/runtime/patch_tool_calls.rs#L86-L156) |
| TodoList | 第一个 | 第一个（stub/noop） | 部分对齐 | Rust 暂不提供 `write_todos` 工具与 todo state/reducer，本阶段仅占位对齐顺序与接口：[todolist_middleware.rs](../../crates/deepagents/src/runtime/todolist_middleware.rs) |
| Memory | 可选（参数 `memory` 非空才启用） | 默认启用（可 `--memory-disable`） | 部分对齐 | Python MemoryState 存在显式 state key；Rust 把 `memory_contents` 放在 `state.private` 并不序列化回传：[state.rs:L6-L20](../../crates/deepagents/src/state.rs#L6-L20)，并写 `state.extra["memory_diagnostics"]`：[memory_middleware.rs:L58-L71](../../crates/deepagents/src/runtime/memory_middleware.rs#L58-L71) |
| Skills | 可选（参数 `skills` 非空才启用） | 默认不启用（需 `--skills-source`） | 部分对齐 | 两边都注入系统提示并提供技能元信息，但 Rust 还拦截“技能工具名”执行 steps，语义更偏工作流；Python 更偏“提示词 + 按需 read_file 打开技能文档”。 |
| Filesystem | 总是启用 | Tool middleware 默认启用 + runtime middleware 占位 | 部分对齐 | Rust `FilesystemRuntimeMiddleware` 当前只做检测与诊断事件（不改写 messages），为未来 `/large_tool_results/...` 引用模板预留接口：[filesystem_runtime_middleware.rs](../../crates/deepagents/src/runtime/filesystem_runtime_middleware.rs) |
| Subagents | 总是启用 | 总是启用 | 基本对齐 | 两边都提供 `task` 工具，但返回/合并策略不同：Python 强依赖 state 必须含 `messages`；Rust 对 child state 有过滤集合，并强制 child 输出非空（见 e2e）。 |
| Summarization | 总是启用 | 默认启用（可 `--summarization-disable`） | 部分对齐 | Python 常见是语义摘要（依赖模型）；Rust 当前 `build_summary_message` 更像预览拼接：[summarization_middleware.rs:L322-L347](../../crates/deepagents/src/runtime/summarization_middleware.rs#L322-L347)，且摘要 message `role="user"`：[summarization_middleware.rs:L339-L346](../../crates/deepagents/src/runtime/summarization_middleware.rs#L339-L346) |
| PromptCaching | 总是启用（Anthropic） | Summarization 之后（stub/noop） | 部分对齐 | Rust 预留对 provider 调用的缓存插桩点，但默认不改变 messages/state：[prompt_caching_middleware.rs](../../crates/deepagents/src/runtime/prompt_caching_middleware.rs) |
| HITL（交互暂停/恢复） | 可选（`interrupt_on`） | 仅策略层，无交互闭环 | 缺失（产品行为差异） | Rust `RequireApproval` 直接返回 tool error，不会暂停等待外部输入：[simple.rs:L723-L762](../../crates/deepagents/src/runtime/simple.rs#L723-L762)；Python CLI 有 HITL interrupt loop 上限与恢复机制：[non_interactive.py:L68-L86](../../../deepagents/libs/cli/deepagents_cli/non_interactive.py#L68-L86) |

## 2) 工具名与输入输出 schema 对齐

> 口径：Python 工具 schema 以 `StructuredTool.from_function` 的参数注解为准；Rust 工具 schema 以 `serde(deny_unknown_fields)` 的输入 struct 与实际输出 JSON 形状为准。

| 工具 | Python：输入 | Python：输出 | Rust：输入 | Rust：输出 | 对齐度 | 关键差异 |
|---|---|---|---|---|---|---|
| write_todos | 由 LangChain todo middleware 定义 | 通常以 state update 形式体现 | 无 | 无 | 缺失 | Rust 未实现 todo state 与写入工具。 |
| ls | `path: str` | `str(result)`（本质是 paths list 的字符串化）[filesystem.py:L522-L557](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L522-L557) | `{path}` [std_tools.rs:L41-L45](../../crates/deepagents/src/tools/std_tools.rs#L41-L45) | JSON 数组（FileInfo）[std_tools.rs:L57-L63](../../crates/deepagents/src/tools/std_tools.rs#L57-L63) | 部分对齐 | 输出形态差异大：Python 返回字符串；Rust 返回结构化 JSON。 |
| read_file | `file_path, offset(int,0-based), limit` [filesystem.py:L564-L568](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L564-L568) | 文本：`str`（cat -n），图片：`ToolMessage` [filesystem.py:L577-L644](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L577-L644) | `{file_path, offset?, limit?}` [std_tools.rs:L70-L78](../../crates/deepagents/src/tools/std_tools.rs#L70-L78) | `{content,truncated,next_offset?}` [std_tools.rs:L80-L85](../../crates/deepagents/src/tools/std_tools.rs#L80-L85) | 部分对齐 | Rust 不支持图片；Python 支持图片并返回多模态块。Rust 输出为 JSON 包裹。 |
| write_file | `file_path, content` [filesystem.py:L673-L677](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L673-L677) | `Command(update=...)` 或错误字符串 [filesystem.py:L669-L736](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L669-L736) | `{file_path, content}` [std_tools.rs:L129-L134](../../crates/deepagents/src/tools/std_tools.rs#L129-L134) | `WriteResult` JSON [std_tools.rs:L146-L152](../../crates/deepagents/src/tools/std_tools.rs#L146-L152) | 部分对齐 | Python 倾向“工具直接写 state”；Rust 返回结果并由 tool middleware 更新 state。 |
| edit_file | `file_path, old_string, new_string, replace_all=False` [filesystem.py:L741-L748](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L741-L748) | `Command(update=...)` 或错误字符串 [filesystem.py:L738-L809](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L738-L809) | `{file_path, old_string, new_string}` [std_tools.rs:L188-L194](../../crates/deepagents/src/tools/std_tools.rs#L188-L194) | `EditResult` JSON [std_tools.rs:L206-L215](../../crates/deepagents/src/tools/std_tools.rs#L206-L215) | 部分对齐 | Python 支持 replace_all 参数；Rust 当前只做全量 replace（`String::replace`），但通过 occurrences 计数体现。 |
| delete_file | 无 | 无 | `{file_path}` [std_tools.rs:L159-L163](../../crates/deepagents/src/tools/std_tools.rs#L159-L163) | `DeleteResult` JSON [std_tools.rs:L175-L181](../../crates/deepagents/src/tools/std_tools.rs#L175-L181) | Rust 额外 | Python default tools 不提供删除工具（通常依赖 edit/覆盖）。 |
| glob | `pattern, path="/"` [filesystem.py:L811-L863](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L811-L863) | `str(list)` | `{pattern}` [std_tools.rs:L222-L226](../../crates/deepagents/src/tools/std_tools.rs#L222-L226) | `Vec<String>` JSON [std_tools.rs:L238-L244](../../crates/deepagents/src/tools/std_tools.rs#L238-L244) | 部分对齐 | Python 有 `path` 参数；Rust 通过 pattern 相对 root 遍历。输出形态也不同。 |
| grep | `pattern (literal), path?, glob?, output_mode=files_with_matches` [filesystem.py:L865-L910](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L865-L910) | `str(...)`（可能格式化 content/计数） | `{pattern, path?, glob?, output_mode?, head_limit?}` [std_tools.rs:L251-L271](../../crates/deepagents/src/tools/std_tools.rs#L251-L271) | 随 output_mode 变化的结构化 JSON [std_tools.rs:L294-L326](../../crates/deepagents/src/tools/std_tools.rs#L294-L326) | 部分对齐 | 两边都是 literal contains，但 Rust 的 output_mode 返回结构化数组；Python 返回字符串。 |
| execute | `command, timeout?` [filesystem.py:L912-L1030](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L912-L1030) | `str`（拼接 exit code/output） | `{command, timeout?}` [std_tools.rs:L333-L339](../../crates/deepagents/src/tools/std_tools.rs#L333-L339) | `ExecResult` JSON [std_tools.rs:L351-L357](../../crates/deepagents/src/tools/std_tools.rs#L351-L357) | 部分对齐 | Rust runtime 与 CLI 都强制走 ApprovalPolicy 决策（deny/require → error tool result）：[simple.rs:L574-L762](../../crates/deepagents/src/runtime/simple.rs#L574-L762)；Python 侧执行策略更多由 CLI/HITL/allow-list 外围提供。 |
| task | `description, subagent_type`（额外可有 response_language 等扩展字段，取决于子代理定义） | 子代理最后文本写回 ToolMessage | `{description, subagent_type}` [subagents/protocol.rs:L23-L28](../../crates/deepagents/src/subagents/protocol.rs#L23-L28) | `{content: "<final_text>"}` [subagents/middleware.rs:L135-L138](../../crates/deepagents/src/subagents/middleware.rs#L135-L138) | 部分对齐 | Python task schema 更开放（支持额外参数）；Rust 当前更严格（deny_unknown_fields）。 |
| compact_conversation | 由 SummarizationMiddleware 提供（工具触发压缩） | 取决于 LangChain summarization middleware 实现 | arguments 被忽略 | `{skipped:true}` 或 `{cutoff_index,file_path?,summary_message,...}` [summarization_middleware.rs:L202-L276](../../crates/deepagents/src/runtime/summarization_middleware.rs#L202-L276) | 部分对齐 | Rust 明确把其作为 runtime 接管的系统级工具；Python 是否暴露/命名对齐依赖上游 middleware。 |

## 3) 状态结构对齐

| state 维度 | Python（deepagents middleware 可见的 key） | Rust（AgentState） | 对齐度 | 备注 |
|---|---|---|---|---|
| messages | 必须存在（多处强依赖） | 由 runtime 入参携带，不在 AgentState 中 | 语义不同 | Rust 把消息序列作为 `run(messages)` 的输入与循环变量，不作为 state 字段。 |
| files / filesystem | `FilesystemState.files: dict[path, FileData]`（reducer 支持删除标记）[filesystem.py:L126-L131](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L126-L131) | `state.filesystem.files: BTreeMap<String, FileRecord>` [state.rs:L22-L40](../../crates/deepagents/src/state.rs#L22-L40) | 基本对齐 | Rust 额外记录 `deleted/truncated`；Python 删除通过 value=None 的 reducer 语义实现。 |
| todos | 由 TodoListMiddleware 维护 | 无 | 缺失 | Rust 需要定义 todo state、合并语义与工具协议。 |
| memory_contents | `MemoryState` 私有 key（middleware 注入/管理） | `state.private.memory_contents`（不序列化）[state.rs:L16-L20](../../crates/deepagents/src/state.rs#L16-L20) | 部分对齐 | Rust 把内容放 private，避免泄漏；Python 用 PrivateStateAttr。 |
| skills_metadata | `SkillsState` 私有 key | `state.extra["skills_metadata"]`（公开）[skills_middleware.rs:L49-L60](../../crates/deepagents/src/runtime/skills_middleware.rs#L49-L60) | 部分对齐 | Rust 明确写入 extra（可回传）；Python 用 PrivateStateAttr。 |
| summarization event | `_summarization_event`（私有） | `state.extra["_summarization_event"]` 等 [summarization_middleware.rs:L12-L16](../../crates/deepagents/src/runtime/summarization_middleware.rs#L12-L16) | 部分对齐 | key 名基本一致，但摘要生成方式不同。 |

## 4) 默认安全策略对齐

| 场景 | Python 默认行为 | Rust 默认行为（CLI/run + runtime） | 对齐度 | 风险/备注 |
|---|---|---|---|---|
| 文件系统 root 边界 | FilesystemBackend `virtual_mode=False` 时 root_dir 不是边界（绝对路径/.. 可绕过）[filesystem.py:L142-L172](../../../deepagents/libs/deepagents/deepagents/backends/filesystem.py#L142-L172) | LocalSandbox 强制 root 边界与 symlink 逃逸防护（测试覆盖） | 不同（Rust 更严格） | Python 生产误用风险更高；Rust 更适合作为默认安全语义。 |
| execute 默认策略 | 取决于 backend 是否支持 + CLI/HITL/allow-list 外围策略；FilesystemMiddleware 会在 backend 不支持时移除 execute | ApprovalPolicy 强制 gate：allow/deny/require；deny/require 都会返回 tool error，不会暂停交互 [simple.rs:L574-L762](../../crates/deepagents/src/runtime/simple.rs#L574-L762) | 不同（产品行为差异） | Rust 在“无人值守”更安全，但缺少交互式批准流程。 |
| 危险命令 pattern | Python CLI 有 allow-list + 额外 Unicode/URL 安全检查链路 | Rust CLI/approval 有危险 pattern 与 allow-list 分段解析 [approval.rs:L58-L96](../../crates/deepagents/src/approval.rs#L58-L96) | 部分对齐 | Python 的 unicode_security 更丰富；Rust 当前重点在 shell。 |

## 5) CLI 行为对齐（交互与可观测性）

| 能力 | Python CLI | Rust CLI | 对齐度 | 备注 |
|---|---|---|---|---|
| HITL 交互批准 | 有 interrupt loop、批量审批与上限保护 [non_interactive.py:L68-L86](../../../deepagents/libs/cli/deepagents_cli/non_interactive.py#L68-L86) | 无交互式批准；require/deny 直接报错 | 缺失 | Rust 若要对齐，需要引入“暂停→询问→恢复”的会话协议（CLI/ACP 都要支持）。 |
| Provider 生态 | LangChain 模型生态（OpenAI/Anthropic 等） | 仅 mock/mock2（脚本）[main.rs:L423-L435](../../crates/deepagents-cli/src/main.rs#L423-L435) | 缺失 | Rust 需要至少一个真实 provider，且 tool-calling 形态需与 ToolSpec 对齐。 |
| 追踪/审计 | Python 有 LangSmith 等集成 | Rust 有 audit sink（JSONL）[main.rs:L790-L813](../../crates/deepagents-cli/src/main.rs#L790-L813) | 部分对齐 | Rust audit 更偏安全审计；Python 追踪更偏链路观测。 |

## 6) 建议的“对齐验收”输出物（可作为后续任务清单）

1. 工具协议对齐：统一输出形态（字符串 vs 结构化 JSON）或在 provider 层做 tool renderer 适配，避免模型端反复解释工具结果。
2. 增补 TodoListMiddleware：补齐 `write_todos` 工具、todo state 与 reducer，并在 SubAgent 隔离/合并规则中对齐 Python 的行为预期。
3. 引入 Tool 结果驱逐（tool-output eviction）：对齐 Python `/large_tool_results/<id>` 机制，减少超大工具输出导致的上下文膨胀。
4. HITL 交互闭环：定义会话级 interrupt/approval 协议（CLI/ACP 都能驱动），实现“暂停→人类输入→继续”。
5. Provider 接入：至少落地一个真实 provider，并对齐 tool-call JSON shape 与 call_id 规则，减少 `normalize_messages` 的误判概率。
