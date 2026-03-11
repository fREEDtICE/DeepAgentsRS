# deepagents Rust 版本技术设计

## 目标与范围

目标是在 `../../deepagents/` 的基础上，提供一套 Rust 实现，放在 `../` 下，并尽量保持“核心抽象与可扩展点”对齐，便于逐步补齐功能与替换运行时。

首阶段（已落地/可运行）聚焦于：

- **Core SDK（deepagents）**：Backends + Tools 的最小可用闭环
- **本地 Sandbox 后端**：提供与 Python FilesystemMiddleware/execute 类似的文件与命令能力
- **可扩展的工具协议**：未来可用于接入 LLM/图执行引擎/中间件链

后续阶段（规划）逐步补齐：

- Middleware 栈（filesystem、subagents、skills、memory、summarization、patch_tool_calls）
- CLI（交互式/非交互式）与 ACP server

## Python 版本架构要点（对齐基准）

Python 版本中核心装配点是 `create_deep_agent()`，其主要工作：

- 解析模型配置与默认子代理配置
- 组装中间件栈（Todo / Memory / Skills / Filesystem / SubAgent / Summarization / PatchToolCalls 等）
- 向上层运行时（LangGraph/LangChain）注册工具与状态 reducer

参考位置：

- SDK 装配：graph.py（`create_deep_agent`）
- Filesystem 工具：middleware/filesystem.py（`ls/read_file/edit_file/write_file/glob/grep/execute`）
- Backend 协议：backends/protocol.py（统一定义文件操作、grep/glob、execute 等）
- CLI 侧的 shell allow-list：deepagents_cli/config.py（`contains_dangerous_patterns` / `is_shell_command_allowed`）

## Rust 版本 Workspace 结构

- `crates/deepagents`：核心库（对齐 Python `libs/deepagents/deepagents`）
- `crates/deepagents-cli`：CLI 入口（对齐 Python `libs/cli/deepagents_cli`，首阶段只保留最小启动骨架）
- `crates/deepagents-acp`：ACP server（对齐 Python `libs/acp/deepagents_acp`，首阶段只保留最小启动骨架）

## Rust 核心抽象设计

### 0) Trait 优先原则（第三方集成优先）

Rust 版本实现应遵守 “Trait 优先” 的设计原则，以便第三方能力（自定义 sandbox、远端执行、定制工具、模型 Provider、审计/追踪、权限策略等）可以通过实现 trait 的方式无缝接入，而不需要 fork 核心库。

具体约束：

- 先定义 trait 与语义契约，再提供默认实现
  - Public API 以 trait 为主，struct 作为默认实现或参考实现
  - trait 的输入/输出语义、错误码与边界条件必须写入契约并有测试覆盖
- 避免把实现细节固化到 public struct
  - 对外暴露尽量使用 `dyn Trait` 或泛型参数（按场景权衡），避免用户被迫依赖某个具体类型
  - 允许用户替换实现：LocalSandbox 只是默认实现，不是唯一实现
- 通过组合而不是继承堆叠能力
  - Middleware、Provider、Runtime、Skills 等均以 trait 表达“可插拔能力”
  - 默认实现仅作为组合示例（参考实现），不应成为依赖入口
- 版本演进以“向后兼容的 trait 扩展”为主
  - 优先增加新 trait 或增加带默认实现的方法
  - 避免破坏性修改（除非大版本），并在兼容矩阵中同步更新验收项

### 1) Backend 抽象（对齐 Python BackendProtocol/SandboxBackendProtocol）

在 Rust 中用 trait 表达能力边界：

- `Backend`：基础健康检查能力
- `FilesystemBackend`：文件与搜索能力
  - `ls_info`：列出目录（结构化 `FileInfo`）
  - `read`：分页读取（cat -n 格式，便于与 “read_file 输出带行号” 的工具习惯对齐）
  - `write_file`：写新文件
  - `edit_file`：字符串精确替换（统计替换次数）
  - `glob`：glob 匹配返回绝对路径列表
  - `grep`：字面量匹配返回结构化 `GrepMatch`
- `SandboxBackend`：在文件能力之上扩展 `execute`

这些 trait 的设计意图是把“存储/执行环境差异”收敛在 backend 内部；上层 tool/middleware 不关心具体实现（本地、远端、容器、云 sandbox）。

#### Backend/Tool 契约（必须明确的语义）

为避免不同实现之间产生不可控差异，本项目约定以下语义为稳定契约：

- Path 语义
  - `root` 是访问边界，所有文件操作必须保证最终解析路径在 `root` 内
  - `file_path/path` 可以是绝对路径或相对路径；相对路径按 `root` 拼接
  - 必须做 normalize（消解 `.`/`..`），并拒绝越界访问
- `read(file_path, offset, limit)`
  - `offset/limit` 以“行”为单位，`offset` 为 0-based，输出行号为 1-based（cat -n 风格）
  - 输出必须稳定包含行号分隔符，便于上层做精确 edit 提示
  - 对超大输出必须截断，并在返回中携带截断信号（见“输出截断与预算”）
- `glob(pattern)`
  - pattern 相对 `root` 生效；若传入以 `/` 开头的 pattern，按相对 `root` 处理（去掉开头 `/`）
  - 返回绝对路径列表，排序稳定
- `grep(pattern, path, glob)`
  - `pattern` 按字面量匹配（非 regex）
  - `path` 若为空则为 `root`；否则必须落在 `root` 内
  - 返回 `GrepMatch { path(绝对), line(1-based), text }`
- `write_file(file_path, content)`
  - 仅“创建新文件”，若已存在返回 `file_exists`
  - 若父目录不存在返回 `parent_not_found`
- `edit_file(file_path, old_string, new_string)`
  - 精确字符串替换（非 regex），返回替换次数 `occurrences`
  - 若 `old_string` 未出现返回 `no_match`

#### 结构化错误模型（对齐 Python 可恢复错误）

为便于上层（CLI/ACP/Agent runtime）对错误做确定性处理，文件类操作使用结构化错误码：

- `file_not_found`：目标不存在
- `parent_not_found`：写入时父目录不存在
- `permission_denied`：越界或权限问题
- `is_directory`：把目录当文件读/写/改
- `invalid_path`：路径非法或无法解析
- `file_exists`：写入时目标已存在
- `no_match`：编辑时未找到 old_string
- `timeout`：执行超时
- `command_not_allowed`：命令未通过 allow-list 校验

要求：

- Backend 层返回结构化错误码（而不是随意字符串），Tool 层原样透传
- CLI/ACP 只做展示与策略控制，不改变错误码含义

### 2) Tool 抽象（对齐 Python StructuredTool/BaseTool）

Rust 版本提供统一 Tool 协议：

- `Tool` trait：`name/description/call(input_json)` 形式
- `ToolResult`：返回 `serde_json::Value`，降低工具协议层与业务 struct 的耦合

这样做的原因：

- 便于未来接入不同的 LLM/Agent runtime（工具调用基本都以 JSON schema/JSON payload 表达）
- 便于用同一套工具服务 CLI、ACP server、或图执行器

### 3) Agent 装配（对齐 Python create_deep_agent）

目前 Rust 的 `create_deep_agent(root)` 聚焦于：

- 创建一个默认 backend（本地 `LocalSandbox`）
- 注册一组默认工具（`ls/read_file/write_file/edit_file/glob/grep/execute`）
- 提供 `call_tool` 便于后续 runtime 或测试直接驱动工具调用

后续扩展方向：

- 引入 Middleware 链，允许像 Python 一样“按能力注入工具 + 拦截/改写请求/响应”
- 使用 typed runtime builder 作为显式装配入口：`DeepAgent::runtime(provider).with_root(...).build()`
- `DeepAgent::run()` 仅保留为兼容入口，并显式返回配置错误，避免“空实现 + 成功返回”的伪合法状态

### 4) Agent Runtime（Rust 版必须补齐的核心）

Python 版 `create_deep_agent()` 的价值不仅是“注册工具”，更关键是其背后依赖的运行时（LangGraph/LangChain）提供了：

- 消息与工具调用协议
- 图/循环调度（模型调用 → 工具调用 → 状态更新 → 下一轮模型调用）
- checkpoint/store 与状态 reducer

Rust 版如果目标是完整替代 `/py`，必须明确 runtime 形态并形成端到端闭环。本项目建议优先采用“纯 Rust runtime”：

- 纯 Rust runtime（推荐默认目标）
  - 用 tokio 驱动消息循环
  - 用强类型 struct/enum 表达 ToolCall/ToolResult/错误码
  - Middleware 以链式 trait 形式工作（请求前/响应后拦截、状态更新）
- 桥接 Python runtime（可选，短期验证）
  - 通过子进程/IPC 调用 Python deepagents
  - 风险：并非等价 Rust 实现；调试与部署复杂度更高

#### 最小 Runtime 闭环（必须做到）

无论采用何种形态，最小闭环要求：

- 输入：用户消息序列
- 处理：Provider 推理产生（可选）tool call
- 执行：tool 执行返回结构化 result/error
- 回填：FilesystemState/MemoryState 等 reducer 合并到 state
- 输出：最终 assistant 消息

#### 当前公共装配形态

当前 public API 已采用更 Rust-shaped 的显式装配路径：

- `DeepAgent`：负责 backend、tools、middleware 组合
- `DeepAgent::runtime(provider)`：进入 typed builder
- `with_root(...)`：作为必填步骤，将 workspace/root 显式绑定到 runtime
- `build()`：产出 `SimpleRuntime`

这样做的目的不是隐藏 wiring，而是把“provider/root 未配置”的非法状态提前到类型与构造阶段，而不是留到运行时才出现静默空行为。

### 5) Provider 抽象（模型接入）

Python 版大量能力由 LangChain 的模型层承担（工具调用、流式、重试、追踪）。Rust 版需要一个最小 provider 抽象来替代该层：

- `Provider` trait（建议）
  - 输入：消息、可用工具列表（含 schema/description）、当前 state 摘要、超时/重试参数
  - 输出：assistant 消息或 tool call（结构化枚举）
- 必备能力
  - timeout/retry（可配置）
  - streaming（可选但建议预留）
  - tool call 结构化输出（不可用则至少支持“函数名+JSON 入参”）

阶段性策略：

- Phase 1.5 使用 mock provider（规则/脚本驱动）验证闭环
- 后续再接入真实模型 provider，并在不改 tool/backends 的情况下替换实现

### 6) Skills/插件机制（Rust 版与 Python 版的最大语义差异点）

Python 版 skills 依赖动态 Python 代码执行；Rust 版不可能直接等价，必须明确插件机制与安全边界。建议优先选择 WASM 作为技能承载：

- 方案 A：WASM 插件（推荐）
  - 技能以 WASM module 形式分发
  - 通过 host ABI 暴露受控能力（调用工具、读写受限 state）
  - 易于沙箱化与跨平台
- 方案 B：声明式技能 + 内置工具
  - skills 只描述“提示词/工具组合/参数模板”
  - 上手快，但与 Python skills 能力差异大
- 方案 C：嵌入脚本引擎（Lua/JS）
  - 灵活但需要额外的沙箱与依赖治理

无论哪种方案，都需要在文档中明确：

- skills 的包结构与元数据（版本、权限、依赖）
- skills 可调用的工具白名单与 root 边界
- skills 的输入输出 schema 与兼容策略

### 7) 关键扩展点清单（建议以 trait 暴露）

为保证“第三方能力可替换”，建议明确并在 core crate 中优先以 trait 形式暴露以下扩展点：

- `FilesystemBackend` / `SandboxBackend`：本地/远端/容器/云 sandbox
- `Tool`：内置工具与外部工具扩展
- `Middleware`：能力编排、拦截、审计、状态更新
- `Runtime`：图/循环调度器（纯 Rust 或桥接）
- `Provider`：模型接入（mock/真实、多厂商）
- `SkillPlugin`：技能加载与执行（WASM/声明式/脚本）

## Phase 1.5 选型结论（Runtime/Provider/SkillPlugin）

- Runtime：默认选择纯 Rust runtime（tokio 驱动的消息循环 + tool-calling 编排），桥接 Python runtime 仅作为对照/短期验证路径
- Provider：以 trait 固化“消息 → step（final/tool_calls/skill_call/error）”的统一接口；Phase 1.5 先用可脚本化 mock provider 验证闭环与可替换性
- Skills：Phase 1.5 默认落地声明式 skills（manifest 驱动）以最小依赖固化插件公共接口；WASM 作为后续演进方向，要求 SkillPlugin trait 不阻碍迁移
- `StateStore`（或 `CheckpointStore`）：状态持久化与回放
- `ApprovalPolicy`：执行审批/allow-list 与策略决策（CLI/服务端共用）
- `Tracer`：追踪与事件上报（console/log/OTel/自定义）

## 本地 Sandbox（LocalSandbox）技术方案

### 文件系统安全边界

- `root` 作为允许访问的根目录
- 对输入路径做 normalize（处理 `.`/`..`），并确保最终路径在 `root` 下
- 对 “父目录不存在” 的写操作返回可理解的错误

### glob/grep 实现策略

- glob：使用 `globset` 编译 pattern，并用 `walkdir` 遍历 `root` 下文件匹配相对路径
- grep：逐文件逐行做 **字面量** `contains`，返回 `GrepMatch {path,line,text}`，并设置最大匹配数上限避免爆量

### execute 与 allow-list

设计上允许执行能力被严格收敛：

- 如果配置了 `shell_allow_list`：仅允许 allow-list 内命令，且拒绝常见危险 shell pattern（重定向、命令替换、裸变量展开、后台执行等）
- 如果未配置 allow-list：库层能力不强制限制；但产品层（CLI/ACP）在非交互/无人值守模式必须默认拒绝或要求审批（deny-by-default）

这与 Python CLI 的策略一致：allow-list 的语义是 “可自动批准/可无审批执行的命令集合”，并通过危险模式检测防注入绕过。

### 输出截断与预算

为避免工具输出造成上层上下文/传输压力，约定：

- `read_file`：默认分页读取；当内容过大需截断时，返回体必须包含“已截断”的可检测信号，并建议用户通过 offset/limit 继续读取
- `execute`：对 stdout/stderr 合并输出设置上限；截断时标记 `truncated=true`
- 未来接入 LLM runtime 时，上层可基于该信号做二次摘要/分页拉取

### 非目标与后续目标

首阶段不覆盖或仅做占位：

- 二进制文件与图片读取（Python 支持图片 content block）；Rust 首阶段仅支持文本
- 全量的 prompt/middleware 行为对齐（以契约与能力闭环优先）

## 当前实现清单（落地状态）

- `deepagents::backends::LocalSandbox`：实现 `FilesystemBackend` + `SandboxBackend`
- `deepagents::tools::default_tools`：注册默认工具集合
- 单元测试：覆盖 read/write/edit、glob/grep、execute allow-list

## 后续对齐路线（建议）

建议按“先定标准与安全口径，再扩能力”的顺序增量对齐：

1. 契约与错误码对齐（本节的契约表与错误码，补齐测试用例集合）
2. FilesystemMiddleware 状态管理（文件快照与 reducer + 工具 schema + 截断策略）
3. CLI 非交互模式安全策略（审批/allow-list 默认口径）
4. ACP server（复用同一套 Tool 协议与错误码）
5. SubAgentMiddleware：`task` 工具 + 子代理注册/路由
6. PatchToolCallsMiddleware：兼容/修补不同 runtime 的 tool call 形态差异
7. SkillsMiddleware：技能加载、schema 校验、工具注册（对齐 CLI 的技能目录约定）
8. MemoryMiddleware：长期/短期记忆存储抽象（对齐 Python memory middleware）
9. SummarizationMiddleware：历史压缩/剪裁策略（可先做接口，后做实现）

在每一步都建议：

- 先定义 trait 与数据结构边界（稳定 API）
- 再做一份本地实现与一组单元测试
- 最后在 CLI/ACP 中串起来形成端到端路径

## 兼容矩阵与验收标准（建议维护）

建议用表格维护“Python 能力点 → Rust 状态 → 验收方式”，用于持续对齐与防止遗漏：

- Filesystem tools：Rust 已有（单测 + CLI 调用示例）
- execute + allow-list：Rust 已有（单测 + 非交互默认策略待落地）
- FilesystemMiddleware state/reducer：未实现（验收：状态合并规则 + 工具输出可回填）
- Subagents：未实现（验收：task 工具 + 子代理声明格式 + 路由测试）
- Skills：未实现（验收：技能加载、schema 校验、工具注册）
- Memory：未实现（验收：存取抽象 + 最小实现 + 行为测试）
- Summarization：未实现（验收：历史裁剪策略 + 回归测试）
- Patch tool calls：未实现（验收：兼容多种 tool call 形态的转换测试）
- CLI：Rust 最小可用（验收：tool 子命令可驱动工具闭环）
- ACP：Rust 骨架（验收：端到端工具调用与会话生命周期）

## 版本更新：对齐 Python 默认中间件顺序

目标：Rust 的“默认顺序”与 Python `create_deep_agent()` 的主 agent 默认栈一致，同时明确“运行时层 vs 工具层”的分层差异。

### Python 默认顺序（主 agent）

`TodoList` →（可选）`Memory` →（可选）`Skills` → `Filesystem` → `Subagents` → `Summarization` → `AnthropicPromptCaching` → `PatchToolCalls` →（可选）用户 middleware →（可选）`HITL`

参考：`create_deep_agent()` 的主 agent 中间件装配：[graph.py:L269-L294](../../deepagents/libs/deepagents/deepagents/graph.py#L269-L294)

### Rust 目标默认顺序（主 agent 对齐版）

`TodoList` →（可选）`Memory` →（可选）`Skills` → `Filesystem` → `Subagents` → `Summarization` → `PromptCaching` → `PatchToolCalls` →（可选）用户 middleware →（可选）`HITL`

说明：
- `PromptCaching` 在 Rust 侧暂时以“可插拔 middleware 接口 + noop 默认实现”对齐语义，后续可接入真实 provider 缓存能力。
- `HITL` 在 Rust 侧需补齐“暂停→询问→恢复”的交互闭环；否则只能视为策略层（不满足对齐目标）。

### 迁移口径（不破坏现有 CLI 的前提）

1. 保持 `SimpleRuntime` 可注入性不变，默认“空 middlewares”仍由上层装配。
2. 在 CLI/ACP 的默认装配路径中，调整 middleware 顺序为目标顺序。
3. 将 Filesystem 相关能力明确拆分为：
   - 工具层（tool middleware）：`FilesystemMiddleware` 负责工具执行后的 state 更新。
   - 运行时层（runtime middleware）：新增 `FilesystemRuntimeMiddleware` 仅用于“工具输出驱逐/系统提示注入”等跨轮逻辑（对齐 Python 语义）。
4. 新增 `TodoListMiddleware` 与 `PromptCachingMiddleware` 的 Rust 版本（可先 stub/noop），以保证顺序和接口对齐，再逐步补功能。

### 版本更新后的验收点（新增）

1. CLI `run` 默认注入顺序与 Python 对齐（主 agent）。
2. `TodoListMiddleware` 与 `PromptCachingMiddleware` 至少具备：
   - 工具名可见、无副作用（noop），不破坏现有逻辑。
3. `FilesystemRuntimeMiddleware` 具备最小能力：
   - 支持“超大工具输出落盘引用”的占位接口（可先返回未启用状态）。
4. `HITL` 保持“策略 + 交互”接口分离，交互版未实现时必须明确提示“不支持交互审批”。

## 版本更新：Phase 8.5–10 对齐进展（实现状态修订）

本节记录在“对齐 Python 默认顺序”之后，Rust 侧已经落地的关键实现与仍待补齐的语义差距，用于修订本文前半部分中“未实现/占位”的历史描述（旧内容保留不删，以便追溯设计演进）。

### 已落地（与对齐目标直接相关）

- 默认顺序装配器：引入 `RuntimeMiddlewareAssembler`，用 slot 固化顺序并在 CLI 默认路径启用（避免人为拼装顺序漂移）。  
  - 装配器：[assembly.rs](../crates/deepagents/src/runtime/assembly.rs)  
  - CLI 装配入口：[main.rs:L479-L574](../crates/deepagents-cli/src/main.rs#L479-L574)
- TodoListMiddleware：`write_todos` 工具已实现（merge/replace、summary gate、输入校验），用于对齐 Python todo 中间件的“工具驱动状态更新”语义骨架。  
  - [todolist_middleware.rs](../crates/deepagents/src/runtime/todolist_middleware.rs)
- FilesystemRuntimeMiddleware + offload：引入运行时层 filesystem middleware，用于注入 offload 配置；实际 offload 在 runtime 执行工具后进行（写入文件并替换为预览+引用），对齐 Python 的“大工具输出落盘引用”方向。  
  - middleware：[filesystem_runtime_middleware.rs](../crates/deepagents/src/runtime/filesystem_runtime_middleware.rs)  
  - SimpleRuntime 执行点：[simple.rs:L568-L679](../crates/deepagents/src/runtime/simple.rs#L568-L679)  
  - Runner 执行点：[resumable_runner.rs:L741-L852](../crates/deepagents/src/runtime/resumable_runner.rs#L741-L852)
- 可中断/可恢复运行：引入 `ResumableRunner`，并在 CLI interactive 模式提供“暂停→approve/reject/edit→继续”的交互闭环。  
  - runner：[resumable_runner.rs](../crates/deepagents/src/runtime/resumable_runner.rs)  
  - CLI 交互循环：[main.rs:L593-L689](../crates/deepagents-cli/src/main.rs#L593-L689)
- 多模态 read_file：支持图片读取并以 base64 image content block 返回（修订本文前文“Rust 首阶段仅支持文本”的历史描述）。  
  - 测试：[multimodal_read_file_phase7.rs](../crates/deepagents/tests/multimodal_read_file_phase7.rs)
- PromptCachingMiddleware：已作为占位 middleware 接入默认顺序（目前仅统计，不提供缓存命中/复用语义）。  
  - [prompt_caching_middleware.rs](../crates/deepagents/src/runtime/prompt_caching_middleware.rs)

### 仍待补齐（与 Python 语义仍不等价）

- 真实 Provider 接入：目前 CLI 仍主要支持 mock/mock2 provider；要对齐 Python 生态，需要至少落地一个真实 provider 并对齐 tool-calling 形态。
- DeepAgent 对外主入口：`DeepAgent::run()` 仍为空实现，集成方主要通过 runner/runtime 使用；如需对齐 Python“单入口装配+运行”，需把 runtime 作为可插拔实现接入 `DeepAgent::run()`。  
  - [agent.rs:L48-L52](../crates/deepagents/src/agent.rs#L48-L52)
- Summarization 语义摘要：当前摘要构建仍以 preview 拼接为主，尚未引入模型语义总结（与 Python 常见体验差距仍在）。  
  - [summarization_middleware.rs:L322-L347](../crates/deepagents/src/runtime/summarization_middleware.rs#L322-L347)

### 需要跟进的工程风险（建议纳入验收）

- `FilesystemRuntimeMiddleware` 事件统计与实际 offload 执行点分离，若用于观测需确保事件能反映真实 offload（避免误报/漏报）。  
  - [filesystem_runtime_middleware.rs:L89-L120](../crates/deepagents/src/runtime/filesystem_runtime_middleware.rs#L89-L120)
