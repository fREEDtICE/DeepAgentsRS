# Phase 2 详细迭代计划（CLI 安全策略与非交互模式：审批/allow-list）

适用范围：本计划面向 [ITERATION_PLAN.md](ITERATION_PLAN.md#L99-L130) 的 Phase 2。目标是在“产品层（CLI/运行入口）”把 `execute` 风险收敛到可控范围：deny-by-default + allow-list/审批，并提供可审计输出且不泄露敏感信息。

以当前代码为 ground truth，Phase 2 的核心能力已落地（本段用于校准文档与实现一致）：

- `ApprovalPolicy` trait + 默认策略：见 [approval.rs](../../crates/deepagents/src/approval.rs)
- CLI 非交互 deny-by-default 与审计脱敏：见 [main.rs](../../crates/deepagents-cli/src/main.rs#L65-L210)
- runtime 路径对 `execute` 同样 enforce 策略（避免仅 tool 模式安全）：见 [simple.rs](../../crates/deepagents/src/runtime/simple.rs)
- ACP 入口同样 enforce（避免 ACP 绕过）：见 [server.rs](../../crates/deepagents-acp/src/server.rs)

## 1. 完成定义（Definition of Done）

Phase 2 完成必须同时满足：

- CLI 非交互模式下对 `execute` 实施稳定策略：未允许命令必拒绝（deny-by-default）
- allow-list/审批策略以 trait 抽象：第三方可替换决策逻辑，而无需 fork deepagents
- 危险 pattern 检测规则与 pipeline/compound operator 行为可回归（有单测与集成测试）
- 审计输出可启用且默认不记录敏感信息（有明确 redaction 策略与测试）
- 对齐 Python 版本“用例集合”的测试集落地（不参考代码细节，仅对齐行为契约）

## 2. 当前系统基线与缺口

### 2.1 现有能力（可复用）

- 后端侧（默认实现）：
  - `LocalSandbox::execute()` 内置 allow-list 校验、危险模式拒绝、并对 `||/&&/|/;` 进行切分后逐段校验
  - 超时支持（timeout 秒）
  - 输出截断标记（`ExecResult.truncated`）
- CLI 侧：
  - 全局参数 `--shell-allow`（可多次传入），用于配置 backend allow-list
  - 非交互工具调用入口 `deepagents tool execute --input ...`

### 2.2 关键缺口（Phase 2 必补）

本阶段剩余工作聚焦于“契约与回归强度”，而非从 0 到 1 的能力建设：

- 用例矩阵与回归强度：进一步扩展/固化危险模式与分段解析的测试覆盖（以行为契约为准）。
- 错误码与消息边界：确保拒绝原因分类码稳定（`approval_required/not_in_allow_list/dangerous_pattern/empty_command/...`），并严格避免泄露敏感信息。

## 3. 核心设计（必须覆盖的细节）

### 3.1 策略抽象：ApprovalPolicy trait（产品层决策）

目标：把“是否允许执行/是否需要审批”的决策逻辑抽象出来，便于第三方接入。

建议数据模型（概念契约，具体字段可按 Rust 风格调整）：

- `ApprovalRequest`
  - `command: String`
  - `cwd_root: String`（root 信息，用于策略判断）
  - `mode: ExecutionMode`（非交互/交互；Phase 2 重点是非交互）
  - `context: Option<Value>`（可选：由调用方传入的上下文，例如 tool name、runtime trace id）
- `ApprovalDecision`
  - `Allow { reason }`
  - `Deny { reason, code }`
  - `RequireApproval { reason }`
- `ApprovalPolicy` trait
  - `decide(req) -> ApprovalDecision`
  - 允许实现方决定：是否启用危险模式检测、allow-list 解析规则、审批阈值

关键约束：

- 该 trait 归属 core crate 公共 API（Trait-first）
- CLI 必须通过该 trait 执行策略（不直接把策略写死在 CLI 或 backend）
- 默认实现只作为参考：例如 `DefaultApprovalPolicy`

### 3.2 非交互模式语义（deny-by-default + 审批策略）

Phase 2 要求“非交互模式下未允许命令必拒绝”，需要固定以下语义：

- 非交互默认：`ApprovalDecision::Allow` 才执行；其余均拒绝
  - `Deny`：直接拒绝并返回 `command_not_allowed`（或更细分 code，例如 `approval_required`）
  - `RequireApproval`：在非交互模式下也拒绝，但错误码/原因应可区分（便于上层提示“需要审批”）
- 交互模式（若纳入）：允许用户确认后执行（但 Phase 2 以非交互为主，交互可作为可选扩展）

建议 CLI 配置项：

- `--execution-mode non-interactive|interactive`（默认 non-interactive）
- `--approve`（仅在 interactive 或显式允许时生效；非交互下建议不提供“自动批准所有”，避免误用）

### 3.3 allow-list 规则与危险 pattern（契约化）

需要把“如何从命令中识别被执行的程序名”与“哪些模式视为危险”固化为契约并测试。

建议规则（对齐当前实现体验，但以行为为准）：

- allow-list 作用在“命令段的第一个 token”（即程序名）
- command segmentation：按 `;`、`|`、`&&`、`||` 分段，逐段校验
- 危险 pattern 例子（至少覆盖）
  - `$(`, `` ` ``, `${`, `$VAR`, `$'...'`
  - 重定向：`>`, `>>`, `<`, `<<`, `<<<`
  - 进程替换：`<(`, `>(`
  - 换行/控制字符：`\n`, `\r`, `\t`, `\0`
  - 单独的 `&`（后台执行）

策略输出需可分类：

- `dangerous_pattern`
- `not_in_allow_list`
- `empty_command`
- `unknown`（兜底）

### 3.4 审计输出（可观测 + 不泄露敏感信息）

审计目标：能在 CI/生产环境回溯“执行了什么、为何允许/拒绝、结果如何”，但默认不泄露敏感信息。

建议审计记录（JSON）字段：

- `timestamp`
- `root`
- `command_redacted`（必须经过脱敏）
- `decision`（allow/deny/require_approval）
- `decision_reason`（短文本）
- `exit_code`（仅 allow 且实际执行后）
- `truncated`（执行输出是否截断）
- `duration_ms`

脱敏策略（建议最小可行且可回归）：

- 默认：不记录原始命令，记录 `command_redacted`
- `command_redacted` 规则：
  - 若出现 `--token/--key/--password/--secret` 等参数，后一个 token 替换为 `***`
  - 若出现 `TOKEN=.../KEY=...` 等形态，`=` 后替换为 `***`
  - 其余 token 原样保留（可读性与可审计性折中）

审计输出开关：

- `--audit-json <path>`：追加写入 JSONL
- 或环境变量 `DEEPAGENTS_AUDIT_JSON=<path>`

### 3.5 与 backend 的职责边界（避免绕过策略）

Phase 2 的关键是“产品层收敛风险”，需要明确策略的生效位置：

- CLI 入口（tool/run）在调用 `execute` 前必须经过 `ApprovalPolicy`
- backend 允许保留自检（防御性），但不能成为唯一防线
- 对外暴露的 API（例如 runtime provider tool-calling）也应能复用同一策略（后续可通过 middleware/工具包装层实现）

## 4. 详细迭代拆解（里程碑）

### M0：契约与配置文档固化

- 输出
  - `ApprovalPolicy` trait 的契约说明：决策输入/输出、错误码映射
  - CLI 配置项与环境变量约定
  - 审计输出格式与脱敏规则
- 验收
  - 文档可作为后续实现与 E2E 的唯一依据

### M1：core crate 增加 approval 模块（Trait-first）

- 任务
  - 新增 `approval` 模块（协议 + 默认实现）
  - 提供 `DefaultApprovalPolicy`（deny-by-default + allow-list + 危险模式检测 + 分段解析）
  - 提供 `CommandParser`/`DangerousPatternDetector` 等可替换小组件（可选，但有利于第三方复用）
- 验收
  - 单测覆盖：危险模式、分段解析、空输入、allow-list 命中/未命中

### M2：CLI 集成（非交互模式 + 审计）

- 任务
  - CLI 增加统一的执行策略配置入口（flag + env）
  - `tool execute` 在执行前调用 `ApprovalPolicy`
  - `run` 模式（Phase 1.5 runtime）触发 execute 时，同样走 `ApprovalPolicy`（避免仅 tool 模式安全）
  - 审计输出：允许/拒绝/执行完成均写记录（拒绝也可审计）
- 验收
  - 集成测试：非交互模式下未允许命令必拒绝
  - 审计记录格式稳定，且脱敏规则可回归

### M3：测试集对齐 Python 用例集合（行为对齐）

- 任务
  - 建立“命令安全用例矩阵”（不引用 Python 代码，只对齐行为）
  - 覆盖：
    - 危险模式拒绝
    - pipeline/compound operator 行为
    - 空输入行为
    - allow-list 正常放行（含 quoting）
    - 审计脱敏（含典型 secret 参数）
- 验收
  - `cargo test` 全量通过
  - 用例矩阵能稳定在 CI 重复执行

## 5. 测试计划（可回归为第一优先级）

### 5.1 单元测试（core）

- `contains_dangerous_patterns` 等价行为（pattern 集合固定）
- 分段解析（`;`/`|`/`&&`/`||`）与“逐段程序名校验”
- quoting 与 tokenization（至少覆盖简单 `'`/`"`）
- 脱敏规则（覆盖参数型与 `KEY=VALUE` 型）

### 5.2 集成测试（CLI）

- 非交互 deny-by-default：无 allow-list 时 execute 必拒绝
- allow-list 生效：允许命令成功执行
- pipeline/compound：每段命令均需允许，否则整体拒绝
- 审计输出：拒绝与执行均产生 JSONL 记录，且不泄露 secret

## 6. 交付物清单（Deliverables）

- 文档
  - Phase 2 详细迭代计划（本文）
  - CLI 配置项与环境变量约定（可作为本文的一部分或独立文档）
  - 审计输出规范（字段与脱敏）
- 代码
  - `ApprovalPolicy` trait + 默认实现
  - CLI 非交互策略集成与审计输出
  - 对齐 Python 行为契约的测试矩阵（单测 + 集成）
