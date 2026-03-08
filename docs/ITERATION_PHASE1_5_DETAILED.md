# Phase 1.5 详细迭代计划（Runtime/Provider/Plugin 选型 + 最小闭环 POC）

适用范围：本计划面向 [ITERATION_PLAN.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/ITERATION_PLAN.md#L57-L87) 的 Phase 1.5。目标是在 Phase 1 已具备的“工具 + filesystem state 回填”基础上，补齐 Rust 版最关键的三项抽象（Runtime / Provider / SkillPlugin），并建立一个可回归的端到端闭环：用户消息 → provider 推理 → tool call → tool 执行 → 结果回填 → 最终响应。

本计划强调：

- 效果参考 Python 版本体验（闭环稳定、可替换 provider、可插拔 skills），但不依赖 Python 代码细节
- Trait-first：公共 API 以 trait 暴露，默认实现仅为参考实现
- 以“可测”为第一优先级：Phase 1.5 的验收以 E2E 可重复执行为准

相关文档：

- Phase 1.5 E2E 计划：[E2E_PHASE1_5.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/E2E_PHASE1_5.md)
- 技术设计基线（含 Runtime/Provider/SkillPlugin 方向）：[TECH_DESIGN.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/TECH_DESIGN.md)
- Phase 1 状态与 schema 基础：[ITERATION_PHASE1_DETAILED.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/ITERATION_PHASE1_DETAILED.md)

## 0. 当前系统基线（Phase 1.5 启动时）

已具备（可复用）：

- Core：`SandboxBackend`/`Tool`/默认工具集（含 `read_file`、`execute` 等）
- FilesystemMiddleware：工具调用后可回填 `AgentState.filesystem`
- CLI：`tool --state-file` 可观测与持久化 state，且错误可结构化输出
- Phase 1 E2E：已落地可执行（state/reducer/schema/安全边界/execute allow-list）

尚缺失（Phase 1.5 必须补齐）：

- Runtime：消息循环/调度器（provider ↔ tool ↔ state ↔ provider）与收敛策略
- Provider 抽象：统一的模型调用 trait（含 tool call 输出），mock provider 实现
- Skills 插件机制：可插拔的技能载入与执行边界（WASM/声明式/脚本三选一）
- 非交互 CLI 端到端入口：面向“输入消息 → 输出最终响应”的 `run`/`chat` 模式

## 1. Phase 1.5 完成定义（Definition of Done）

Phase 1.5 完成必须同时满足：

- 选型结论落地：
  - Runtime 形态（纯 Rust or 桥接 Python）在文档中明确，并写入 TECH_DESIGN
  - Skills 插件机制（A/B/C）在文档中明确，并给出迁移路径与非目标
- Trait 清单固化为 core crate 公共 API：
  - `Runtime`、`Provider`、`SkillPlugin` 的 trait 边界与最小数据模型（tool call、消息、错误）
- POC 最小闭环可运行（默认使用 mock provider）：
  - 输入：消息
  - 过程：provider 决策产生 tool call → 执行 tool → middleware 回填 state → provider 基于结果给出最终文本
  - 输出：最终响应（可选携带 tool_calls/tool_results/trace）
- 可回归测试：
  - E2E：至少覆盖 [E2E_PHASE1_5.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/E2E_PHASE1_5.md) 中的“最小闭环 + provider 可替换 + tool call 解析鲁棒 + 错误/超时 + plugin 失败隔离（按选型）”
  - 能证明：不改 tool/backends 的情况下替换 provider（mock ↔ provider stub）

## 2. 核心设计决策（必须先定，避免返工）

### 2.1 Runtime 形态：默认选择与边界

建议默认目标：**选项 A：纯 Rust runtime**。理由：

- 更接近“等价 Rust 实现”，避免 Phase 2+ 再迁移造成大返工
- 能直接复用 Rust 侧 state/middleware/tool，减少跨语言同步成本

选项 B（桥接 Python runtime）作为“对照路径/短期验证”存在，但必须明确非目标：

- 不承诺性能/部署简化
- 不承诺与 Rust state/middleware 深度融合

决策输出：

- 新增/更新文档：在 TECH_DESIGN 增加 Phase 1.5 选型结论与边界（写明为何 A、何时考虑 B）
- E2E：至少覆盖纯 Rust runtime；若实现桥接则补充对照用例

### 2.2 ToolCall 统一协议（对齐“可替换 provider”）

必须固化一份 provider → runtime 的 tool call 协议（与工具本身 JSON input 对齐）：

- `tool_name: String`
- `arguments: serde_json::Value`
- `call_id: Option<String>`（用于关联 tool_results；若缺省则 runtime 生成）

tool result 协议：

- `call_id: Option<String>`
- `tool_name: String`
- `output: serde_json::Value`
- `error: Option<String>`（结构化错误码或可分类字符串）

要求：

- runtime 对 tool call 做 schema 校验（缺字段/错类型/未知字段/未知工具）
- 错误语义在输出中可断言（便于 E2E 与上层集成）

### 2.3 Provider trait：最小可替换面

Provider 在 Phase 1.5 必须支持两类输出：

- 直接输出最终文本（不触发工具）
- 输出 tool call（触发一次或多次）

建议的最小 trait 形态（概念层，不绑定实现细节）：

- 输入：messages、可用工具清单（名称/描述/可选 schema 摘要）、当前 state 摘要、超时/重试参数
- 输出：`ProviderStep`（枚举：FinalText / ToolCalls / Error）

并明确：

- timeout：provider 调用必须可超时
- retry：Phase 1.5 可先实现“固定次数重试”或“无重试但预留接口”，但必须写清楚
- streaming：Phase 1.5 允许不实现，但必须预留接口或声明非目标

### 2.4 Runtime trait：循环与收敛策略

Runtime 的职责是把 provider 与 tool 执行编排成循环，并明确收敛策略：

- 最大轮数（避免死循环）
- tool 失败时的策略（两选一并固化）：
  - A：返回结构化错误并终止
  - B：将 tool 错误回填给 provider，让 provider 生成最终解释文本

建议 Phase 1.5 默认采用 B（更贴近 Python 体验），但必须通过 E2E 固化行为。

### 2.5 Skills 插件机制：Phase 1.5 的落地策略

Phase 1.5 “必须选一个”，同时要考虑工程可落地与后续演进：

- 选项 A：WASM 插件（推荐长期目标）
- 选项 B：声明式技能 + 内置工具（最易落地）
- 选项 C：脚本引擎（Lua/JS）

建议 Phase 1.5 采用分层策略（仍满足“必须选一个”）：

- **本阶段默认落地选项 B（声明式技能）**，原因：Phase 1.5 的关键是 runtime/provider/tool 闭环与契约固化，声明式技能能以最少依赖把“插件机制的公共接口”先固化下来
- 同时在文档中把选项 A（WASM）定义为后续演进路径，并保证 Phase 1.5 的 `SkillPlugin` trait 设计不阻碍迁移

如果团队希望 Phase 1.5 就落地 WASM，则需要把“WASM 运行时依赖选型与 ABI”纳入本阶段里程碑（见 3.4 的分支任务）。

## 3. 详细迭代拆解（里程碑）

### M0：选型结论与契约固化（文档优先）

- 任务
  - 在 TECH_DESIGN 增补 Phase 1.5 三项选型结论（Runtime/Provider/SkillPlugin）与 trade-off
  - 明确非目标、迁移路径与兼容策略（尤其是从声明式 skills → WASM 的迁移）
  - 固化 tool call/tool result 的统一协议（字段名、错误字段、call_id 规则）
- 验收
  - TECH_DESIGN 更新完成，且与 E2E_PHASE1_5 用例一致

### M1：core crate 新增 protocol 层（类型与 trait）

- 任务
  - 新增 `runtime`/`provider`/`skills` 三个模块（仅协议与 trait）
  - 固化最小数据模型：
    - Message/Role（可复用现有 `types::Message`，必要时扩展为 enum）
    - ToolCall / ToolResult（结构化）
    - ProviderRequest / ProviderStep / ProviderError（结构化）
    - RuntimeConfig（max_steps/timeout 等）
  - 增加 serde 校验策略：对来自 provider 的 tool call 必须做严格校验（缺字段/错类型/未知字段）
- 验收
  - core crate 编译通过
  - trait 边界明确：第三方可实现 Provider/SkillPlugin/Runtime

### M2：Mock provider（可脚本化、确定性）

- 任务
  - 实现 `MockProvider`：
    - “场景脚本”：输入消息 → 输出 step 序列（FinalText 或 ToolCall）
    - 可配置：超时、错误、malformed tool call、两轮 tool call
  - 实现 Provider 替换示例：`MockProvider2`（同语义不同形态），用于证明 runtime 的兼容性
- 验收
  - 单测：mock provider 场景驱动稳定
  - 集成测试：用不同 provider 跑同一个闭环脚本，输出一致

### M3：最小 Runtime（纯 Rust）闭环实现

- 任务
  - 实现 `SimpleRuntime`：
    - 输入 messages + provider + agent（工具执行入口）
    - 循环：provider.step → tool call → 执行 tool（调用现有 agent/middleware/state）→ 形成 tool_result → 追加到 messages/state → 下一轮 provider
    - 收敛：provider 返回 FinalText 或达到 max_steps
  - trace 输出：为 E2E 提供可断言信息（tool_calls/tool_results/终止原因）
- 验收
  - 集成测试：输入消息触发 read_file，最终响应包含 README 第一行（或等价信息）
  - 负向：malformed tool call/unknown tool 能稳定报错或回填并收敛

### M4：Skills 插件机制 MVP（按选型落地）

#### 默认分支（B：声明式技能）

- 任务
  - 定义 skills manifest（JSON/YAML 二选一；建议 JSON，便于 serde）：
    - skills 列表：name/description/trigger（关键词或显式调用）/tool templates
    - 权限：可调用工具白名单（至少限制 execute）
  - 实现 `DeclarativeSkillPlugin`：
    - 加载 manifest
    - 向 runtime 暴露 skills 列表（用于 provider 决策或直接 runtime 触发）
    - 执行时只能生成 tool call（不直接触碰 FS/execute），由 runtime 统一走 tool 执行
  - 失败隔离：manifest 无效/技能执行失败不应导致 runtime 崩溃
- 验收
  - E2E：加载一个技能（如 “read_readme”）能触发 read_file 并回填
  - E2E：manifest 错误能结构化报错且不崩溃

#### 可选分支（A：WASM 插件，若 Phase 1.5 即落地）

- 任务
  - 选型并引入一个 WASM runtime（需要在实现时确认依赖生态与安全边界）
  - 定义最小 ABI：
    - 列出 skills 元信息
    - 传入输入 JSON，返回 tool calls 或 final text
  - 资源限制：超时/内存限制/禁止直接系统调用（取决于 runtime）
- 验收
  - E2E：加载 wasm 插件并执行一个简单 skill
  - E2E：插件 trap/超时被隔离，runtime 不崩溃

### M5：CLI 端到端入口（run 模式）与 E2E 落地

- 任务
  - CLI 增加非交互 `run` 子命令：
    - `--provider mock/mock2/real/bridge`
    - `--input <message>`（或 `--input-file`）
    - `--root <root>`（复用现有 root 逻辑）
    - `--plugins-dir/--plugin`（按选型）
    - 输出 JSON：`{ final_text, tool_calls, tool_results, error, trace }`
  - 落地 Phase 1.5 E2E 测试套件（见 E2E_PHASE1_5）
- 验收
  - `cargo test` 全量通过
  - Phase 1.5 E2E 覆盖最小闭环、provider 替换、tool call 解析鲁棒、错误/超时、plugin 分支用例

## 4. 测试计划（与验收强绑定）

### 4.1 单元测试（必须）

- Provider：mock 场景脚本解析与超时/错误分支
- ToolCall 解析：缺字段/错类型/未知字段/未知 tool
- Skills（按选型）：manifest 校验或 wasm 加载失败隔离

### 4.2 集成测试（必须）

- runtime + mock provider + agent：read_file 闭环
- provider 替换：mock ↔ mock2 输出行为一致

### 4.3 E2E（必须）

基线参考：

- [E2E_PHASE1_5.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/E2E_PHASE1_5.md)

建议落地位置：

- `crates/deepagents-cli/tests/e2e_phase1_5_runtime.rs`

## 5. 风险与对策

- 风险：选型摇摆导致返工
  - 对策：M0 先固化选型与非目标；E2E 按选型固定输出契约
- 风险：provider/tool call 协议不稳定导致后续接入困难
  - 对策：tool call/tool result 统一协议先落地为强类型 struct + 负向测试
- 风险：plugin 机制引入过多依赖拖慢闭环
  - 对策：Phase 1.5 默认落地声明式 skills，WASM 作为后续演进路径；若必须 WASM，则作为分支里程碑并明确依赖选型与资源限制

## 6. 输出物清单（Deliverables）

- 文档
  - Phase 1.5 E2E 计划：[E2E_PHASE1_5.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/E2E_PHASE1_5.md)
  - Phase 1.5 详细迭代计划（本文）
  - TECH_DESIGN 增补：Phase 1.5 三项选型结论与契约
- 代码
  - core crate：Runtime/Provider/SkillPlugin trait 与协议类型
  - POC：SimpleRuntime + MockProvider + 单工具闭环（read_file）
  - CLI：run 子命令（非交互）与结构化 JSON 输出
  - 回归测试：Phase 1.5 E2E 全套落地
