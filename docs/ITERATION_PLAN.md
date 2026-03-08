# deepagents Rust 版本迭代计划（分阶段）

## 计划原则

- 先定契约：API 语义、错误码、安全口径先稳定，再扩能力
- 先闭环后丰富：每阶段都必须形成可运行/可测试的闭环与验收标准
- 分层不打架：Backend 负责环境差异；Tool 负责对外协议；Middleware 负责能力编排；CLI/ACP 负责产品形态与安全策略
- Trait 优先：每个能力域先定义 trait 与契约，再提供默认实现与集成示例

## Phase 0（已完成）：最小工程骨架 + 本地工具闭环

### 目标

- 搭好 Rust workspace 与核心 crate
- 提供本地 sandbox 后端与一组标准工具，能通过 CLI 驱动并具备单测

### 范围

- `deepagents`：Backend/Tool 协议与 `LocalSandbox`
- `deepagents-cli`：最小 `tool` 子命令
- `deepagents-acp`：最小启动骨架

### 交付物

- Core crate：`FilesystemBackend` + `SandboxBackend` + 默认工具集
- 单元测试：覆盖 read/write/edit、glob/grep、execute allow-list
- 技术设计：[TECH_DESIGN.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/TECH_DESIGN.md)

### 验收

- `cargo test` 全通过
- CLI `tool` 可调用 `ls/read_file/write_file/edit_file/glob/grep/execute`

## Phase 1：契约固化 + FilesystemMiddleware（状态与 schema）

### 目标

- 让“工具调用结果”可以稳定回填为 agent state（为后续 subagents/总结/记忆打基础）

### 范围

- 定义统一的工具输入输出 schema（可选：JSON Schema 或 Rust struct + serde 校验）
- 引入 FilesystemMiddleware 的 state 与 reducer（对齐 Python 的 files snapshot 合并语义）
- 补齐输出截断策略与一致的格式约束（read/execute）

### 交付物

- `FilesystemState` 与 reducer（支持删除标记与合并）
- tools 与 middleware 的 schema 文档与测试
- 端到端示例：一次 tool 调用更新 state，并可再次读取使用

### 验收

- 单测：state reducer 行为（覆盖覆盖、删除、并发更新）
- 集成测试：tool 调用→state 更新→后续 tool 使用该 state

## Phase 1.5：Runtime/Provider/Plugin 选型与最小闭环 POC（关键）

### 目标

- 明确 Rust 版是否“纯 Rust runtime”还是“桥接 Python runtime”，避免后续大返工
- 固化三项基础抽象：Agent runtime、LLM provider、skills 插件机制
- 建立一个最小端到端闭环：消息 →（provider 推理）→ tool call → tool 执行 → 结果回填

### 范围

- Runtime 形态（必须明确并写入 TECH_DESIGN）
  - 选项 A：纯 Rust runtime（推荐默认目标）
  - 选项 B：桥接 Python runtime（短期可用但不算“等价 Rust 实现”）
- Provider 抽象：统一的模型调用 trait（请求/响应、工具调用、流式、重试/超时）
- Skills 插件机制（必须选一个）
  - 选项 A：WASM 插件（推荐：安全边界清晰、跨平台、可沙箱）
  - 选项 B：只支持声明式技能 + 内置工具（实现最快，但与 Python 生态差异大）
  - 选项 C：嵌入脚本引擎（Lua/JS），风险与复杂度中等

### 交付物

- 文档：三项选型结论与 trade-off、迁移路径、非目标
- POC：最小 runtime + mock provider（不依赖真实模型）+ 单个工具闭环（如 `read_file`）
- 回归测试：保证 tool call 解析与执行闭环可重复
- Trait 清单：将 Runtime/Provider/SkillPlugin 的 trait 边界固化为 core crate 公共 API

### 验收

- 端到端测试通过：输入消息 → 触发 tool → tool 输出 → runtime 收敛为最终响应
- 能在不改 tool/backends 的情况下替换 provider（mock ↔ 真 provider）

### 迭代 E2E 测试计划（Phase 1.5）

- 计划文档：[E2E_PHASE1_5.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/E2E_PHASE1_5.md)
- 可执行入口：`deepagents run ...`（非交互、stdout 输出 `RunOutput` JSON，失败退出码非 0）
- 测试工程：`crates/deepagents-cli/tests/e2e_phase1_5_runtime.rs`（spawn CLI 做黑盒断言）
- 迭代门禁（建议）
  - I1（闭环基线）：最小闭环、provider 替换、timeout 分类、tool 错误可回填、skill plugin 触发 tool
  - I2（鲁棒性）：unknown tool、schema 负测、路径越界、max_steps_exceeded、无 tool 直答
  - I3（state 可用性）：write/edit/delete 驱动 state 演进并在 run 输出可观测
- 核心契约点：输出字段与 error.code 分类、call_id 关联规则、MockProvider 脚本 step 索引语义、skills manifest merge_args 覆盖行为

## Phase 2：CLI 安全策略与非交互模式（审批/allow-list）

### 目标

- 在产品层把 execute 风险收敛到可控范围（deny-by-default + allow-list/审批）

### 范围

- CLI 非交互模式：审批策略、allow-list、危险 pattern 校验
- 命令执行的记录与可审计输出（不记录敏感信息）

### 交付物

- CLI 配置项与环境变量约定
- allow-list 解析与校验测试集（对齐 Python 的用例集合）
- `ApprovalPolicy` trait：把“是否允许执行/是否需要审批”的决策逻辑抽象出来，便于第三方策略接入

### 验收

- 单测：危险模式拒绝、pipeline/compound operator 行为、空输入行为
- 集成测试：非交互模式下未允许命令必拒绝

### 迭代 E2E 测试计划（Phase 2）

- 计划文档：[E2E_PHASE2.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/E2E_PHASE2.md)
- 必测入口：`deepagents tool execute --state-file ...` 与 `deepagents run ...`（避免 tool/run 绕过策略）
- 关键契约点：non-interactive deny-by-default、allow-list 分段校验、危险模式分类码、审计 JSONL 与脱敏规则
- 迭代门禁（建议）
  - I1：deny-by-default + 审计/脱敏基线
  - I2：危险模式矩阵 + pipeline/compound + 空输入
  - I3：run 路径不绕过 + 配置优先级/allow-list 来源

## Phase 3：ACP server（端到端会话与工具调用）

### 目标

- 提供可用的 ACP 服务端：会话、消息、工具调用、结果回传

### 范围

- ACP 协议最小子集（与现有 Python ACP 行为对齐的关键路径）
- 复用 Phase 1/2 的 tool schema 与错误码
- 以 trait 形式隔离传输层与业务层（会话存储、认证、限流/审计作为可插拔组件）

### 交付物

- ACP server 可运行与基础集成测试
- 示例：通过 ACP 调用 `read_file/grep/execute` 并返回结构化结果

### 验收

- 端到端测试：建立会话→发起工具调用→返回结果→关闭会话

### 详细计划与 E2E（Phase 3）

- 详细迭代计划：[ITERATION_PHASE3_DETAILED.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/ITERATION_PHASE3_DETAILED.md)
- 黑盒 E2E 测试计划：[E2E_PHASE3_ACP.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/E2E_PHASE3_ACP.md)
- 关键契约点：会话生命周期、工具调用结构化输出（output/error/state/delta/state_version）、复用 Phase 1/2 错误码与 execute 安全/审计

## Phase 4：SubAgentMiddleware（task 工具与子代理路由）

### 目标

- 支持 “Task tool / 子代理” 的注册与调用，为多代理协作铺路

### 范围

- 子代理规范（名称、描述、工具、权限边界）
- 子代理调用的 state 隔离与合并策略

### 交付物

- `task` 工具的最小实现（可调用内置子代理并返回结果）
- 子代理 registry 与路由策略

### 验收

- 测试：子代理调用不越权（根目录/命令权限），结果可控合并

## Phase 5：PatchToolCallsMiddleware（兼容层）

### 目标

- 兼容不同 runtime/协议的 tool call 形态差异，减少上层集成成本

### 范围

- Tool call/response 的归一化、ID 清洗、错误字段兼容

### 交付物

- 转换器与回归测试用例集合

### 验收

- 给定多种输入形态，输出标准形态一致且可 round-trip

## Phase 6：SkillsMiddleware（技能加载与校验）

### 目标

- 对齐 Python/CLI 的 skills 目录约定，实现技能动态加载与工具注册

### 范围

- skills 包结构、元数据、校验规则
- 技能工具注入与权限边界
- `SkillPlugin` trait：抽象技能加载/执行，支持 WASM/声明式/脚本三种实现并存

### 交付物

- skills loader 与校验器
- 示例技能（最小）：一个只读工具 + 一个写文件工具（受控）

### 验收

- 单测：非法技能包拒绝、schema 缺失拒绝、权限越界拒绝

## Phase 7：MemoryMiddleware（记忆抽象与最小实现）

### 目标

- 提供可插拔的记忆存储接口与最小落地（本地/文件）

### 范围

- memory store trait、序列化格式、生命周期与容量策略

### 交付物

- 最小本地实现与测试

### 验收

- 单测：写入/查询/淘汰策略正确

## Phase 8：SummarizationMiddleware（历史压缩）

### 目标

- 提供历史裁剪/摘要接口与策略（先接口后实现）

### 范围

- 可配置策略：按 token/字符预算、按轮次、按重要性

### 交付物

- 接口与空实现 + 回归测试（保证不破坏 tool call 语义）

### 验收

- 关键路径回归：带工具调用的对话不会被裁剪到不可恢复

## Phase 9：统一对齐与发布准备

### 目标

- 梳理兼容矩阵、文档、示例与 CI，形成可持续迭代基线

### 范围

- 兼容矩阵维护、CI、示例工程、版本号与变更日志策略
- Trait 稳定性策略：为公共 trait 增加版本化说明与破坏性变更准则

### 验收

- CI 稳定、示例可跑、文档可按阶段复现

## Parity Matrix（py → rust 对齐矩阵，建议持续维护）

| 能力域 | py 版本位置 | rust 目标形态 | rust 当前状态 | 验收方式 |
|---|---|---|---|---|
| Agent 装配/入口 | `libs/deepagents/deepagents/graph.py` | `create_deep_agent()` 组装 runtime+middleware+tools | 仅基础装配（无 runtime） | Phase 1.5 端到端测试 |
| Backend 协议 | `backends/protocol.py` | traits + 结构化错误码 | 已有（需进一步结构化） | 单测 + 契约表回归 |
| 本地 sandbox | `backends/local_shell.py` 等 | `LocalSandbox` + root 约束 | 已有 | 单测 |
| Filesystem tools | `middleware/filesystem.py` | 默认工具集 + schema | 已有（需 schema/截断信号强化） | 单测 + CLI 调用 |
| Filesystem state/reducer | `middleware/filesystem.py` | `FilesystemState` + reducer | 未实现 | Phase 1 |
| Execute 安全策略 | `deepagents_cli/config.py` | CLI/ACP deny-by-default + allow-list | 部分（库有 allow-list，CLI 策略未落地） | Phase 2 集成测试 |
| CLI（交互/TUI） | `deepagents_cli/app.py` 等 | 非交互优先，TUI 后置 | 最小 tool 子命令 | Phase 2/9 |
| ACP server | `libs/acp/deepagents_acp/server.py` | Rust ACP 最小子集 | 骨架 | Phase 3 端到端 |
| Subagents | `middleware/subagents.py` | task 工具 + registry + 路由 | 未实现 | Phase 4 |
| Patch tool calls | `middleware/patch_tool_calls.py` | 兼容层 | 未实现 | Phase 5 回归测试 |
| Skills | `middleware/skills.py` + CLI skills loader | WASM/声明式/脚本插件 | 未选型 | Phase 1.5/6 |
| Memory | `middleware/memory.py` | store trait + 最小实现 | 未实现 | Phase 7 |
| Summarization | `middleware/summarization.py` | 策略接口 + 回归 | 未实现 | Phase 8 |
