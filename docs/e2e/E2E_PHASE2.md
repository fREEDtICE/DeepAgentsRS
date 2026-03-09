# Phase 2 迭代 E2E 测试计划（CLI 安全策略与非交互模式：审批/allow-list）

适用范围：本计划面向 [ITERATION_PLAN.md](../iteration/ITERATION_PLAN.md) 的 Phase 2。目标是在“产品层（CLI/运行入口）”把 `execute` 风险收敛到可控范围（deny-by-default + allow-list/审批），并提供可审计输出且默认不泄露敏感信息。

本计划以“可执行、可回归、可审计”为第一优先级：所有测试用例必须能在本地/CI 重复执行，且不依赖网络。

## 0. 当前系统情况（Phase 2 E2E 需要基于的现实）

本节只陈述“对第三者可观察”的事实（通过 CLI help/输出 JSON/文件副作用可验证），不依赖任何内部实现细节。

### 0.1 对外可观察入口（可复用）

- CLI 顶层能力（当前版本）
  - 全局选项：`--root`、`--shell-allow`（可多次提供）
  - 子命令：`tool`、`run`
- `tool` 子命令（非交互、可脚本化）
  - 入参：`<NAME>` + `--input <json>`
  - 可选：`--state-file <path>`（用于输出稳定的结构化 JSON，失败也会打印 JSON）
- `run` 子命令（非交互、可脚本化）
  - 入参：`--input <text>`
  - 可选：`--provider`、`--mock-script`、`--plugin`、`--max-steps`、`--provider-timeout-ms`

### 0.2 Phase 2 对外能力缺口（以行为为准）

Phase 2 的交付物包含“审批策略/非交互 deny-by-default/审计输出”等产品层能力，而当前 CLI 对外接口尚未提供对应开关与可观测输出（例如 execution-mode、审计 JSONL）。因此本测试计划将这些作为 Phase 2 的目标行为，并明确 E2E 需要验证的外部契约。

这份计划与 Phase 2 详细设计保持一致（见 [ITERATION_PHASE2_DETAILED.md](../iteration/ITERATION_PHASE2_DETAILED.md)），但所有验收与断言均以“CLI 外部行为”为准：退出码、stdout JSON、文件副作用、审计文件内容。

## 1. 目标与完成定义（E2E 角度）

### 1.1 目标（E2E 要证明的外部行为）

- 非交互模式（默认）：未被允许的命令必拒绝（deny-by-default）。
- allow-list 与危险模式检测规则可回归（含 pipeline/compound 行为、空输入行为）。
- 策略对外生效一致：CLI/tool/run 对 `execute` 的行为一致（不允许某条入口绕过策略）。
- 审计输出可启用：允许/拒绝都可审计；默认不泄露敏感信息（脱敏规则可回归）。

### 1.2 完成定义（门禁）

- `cargo test -p deepagents-cli` 可重复执行通过；所有用例使用临时 root 与临时审计文件，无外部依赖。
- E2E 覆盖下文“必测用例组”（6.1~6.8），并与 Phase 2 验收条目逐条对应：
  - 危险模式拒绝、pipeline/compound operator 行为、空输入行为（单测 + 集成）
  - 非交互模式下未允许命令必拒绝（集成/E2E）

## 2. 术语与契约（Phase 2 的测试必须先把这些写死）

### 2.1 ExecutionMode

- `non_interactive`：默认模式。只有 `Allow` 才允许执行；`Deny/RequireApproval` 均拒绝。
- `interactive`：可选扩展（Phase 2 不作为必测入口），允许用户确认后执行。

### 2.2 ApprovalDecision 与分类码（用于断言）

建议最小分类码（来自 Phase 2 详细计划）：[ITERATION_PHASE2_DETAILED.md](../iteration/ITERATION_PHASE2_DETAILED.md#L96-L102)

- `empty_command`
- `dangerous_pattern`
- `not_in_allow_list`
- `approval_required`
- `unknown`（兜底）

E2E 要求：当 `execute` 被拒绝时，必须能从结构化输出中稳定读到上述分类码之一（不能只靠模糊字符串匹配）。

### 2.3 “不记录敏感信息”的最小承诺

- 审计输出不记录原始命令，只记录 `command_redacted`。
- `command_redacted` 需要做最小脱敏（参数型 + `KEY=VALUE` 型），并有测试覆盖。参见 [ITERATION_PHASE2_DETAILED.md](../iteration/ITERATION_PHASE2_DETAILED.md#L118-L125)。

### 2.4 与 Python 行为对齐说明（可观察语义）

Python 版本的 `execute` 在部分失败场景会返回“可读错误字符串”而非结构化错误；Rust 版本在产品层引入审批/审计后，建议以结构化 `error.code/error.message` 为主以便脚本化断言，但必须保证：

- `error.code` 为稳定枚举（本计划第 2.2 节），入口不同不改变语义
- `error.message` 仍保持可读、可诊断（便于对齐 Python 体验与排障），且不包含敏感信息（参见第 2.3/5.2 节）

## 3. 测试入口与 Harness（E2E 如何驱动）

Phase 2 的 E2E 要覆盖两条入口，避免“tool 安全但 run 绕过”或反之。

### 3.1 CLI tool 入口（推荐：固定输出 JSON）

为保证失败路径也能断言，Phase 2 E2E 推荐使用：

- `deepagents tool execute --state-file <path> --input '{"command":"..."}'`

原因：带 `--state-file` 的 `tool` 子命令在成功与失败时都输出结构化 JSON（便于黑盒断言），并在失败时使用非 0 退出码。

Phase 2 要求：即使 `execute` 在“策略层”被拒绝，也必须输出稳定 JSON，供 E2E 断言（至少包含分类码与审计信息是否写入）。

### 3.2 CLI run 入口（闭环路径，不允许绕过策略）

使用 `deepagents run --provider mock --mock-script ...`，让 mock provider 触发 `execute` tool_call，并断言：

- `RunOutput.tool_calls/tool_results/error/trace` 的行为不被绕过
- 策略拒绝仍会被记录为 tool_result.error（或结构化字段），并触发审计记录

E2E 只依赖 run 的 stdout JSON（例如 final_text/tool_calls/tool_results/state/error/trace 等字段），不依赖其内部调度方式。

### 3.3 统一 Fixture

每个用例使用 `tempfile::tempdir()` 创建独立 root，并准备：

- `out.txt`（初始不存在，用于验证重定向危险模式）
- `audit.jsonl`（临时审计文件路径）
- `state.json`（tool 模式用于稳定 JSON 输出）

## 4. CLI 配置项与环境变量（E2E 必须覆盖的行为契约）

Phase 2 文档要求提供 CLI 配置项与环境变量约定。本 E2E 计划从黑盒角度要求：当这些开关被提供后，其行为必须稳定可断言（名称可实现时微调，但契约不可漂移）。

- ExecutionMode
  - `--execution-mode non-interactive|interactive`（默认 non-interactive）
- allow-list 来源（至少一种，推荐两种）
  - `--shell-allow <cmd>`（可多次传入，已存在）
  - `--shell-allow-file <path>`（建议新增：一行一个命令名，忽略空行/注释）
  - 或环境变量：`DEEPAGENTS_SHELL_ALLOW` / `DEEPAGENTS_SHELL_ALLOW_FILE`
- 审计输出开关
  - `--audit-json <path>` 或 `DEEPAGENTS_AUDIT_JSON=<path>`

E2E 需要覆盖“配置优先级”：

- CLI flag > env > 默认值

## 5. 审计输出（JSONL）契约与断言点

### 5.1 审计记录字段（最小集合）

以 Phase 2 详细计划为准：[ITERATION_PHASE2_DETAILED.md](../iteration/ITERATION_PHASE2_DETAILED.md#L107-L117)

- `timestamp`
- `root`
- `command_redacted`
- `decision`（allow/deny/require_approval）
- `decision_reason`（短文本）
- `exit_code`（仅 allow 且实际执行后）
- `truncated`
- `duration_ms`

E2E 断言策略：

- 校验字段存在与类型；对 `timestamp/duration_ms` 不做精确值匹配，只做“可解析 + 合理范围”（例如 `duration_ms >= 0`）。

### 5.2 脱敏规则（必须可回归）

必须覆盖两类：

- 参数型：`--token abc` / `--password abc` / `--secret abc` / `--key abc`（后一个 token 替换为 `***`）
- 赋值型：`TOKEN=abc` / `KEY=abc` / `PASSWORD=abc`（`=` 后替换为 `***`）

E2E 必须断言：

- 审计文件中不出现原始 secret 字符串
- `command_redacted` 保留足够上下文（能看出是哪个命令、哪个参数被脱敏）

## 6. E2E 用例清单（按能力域分组）

本清单以“迭代门禁”方式组织：先保证 deny-by-default 与审计，再补齐危险模式矩阵与 pipeline/compound，最后扩展到 run 路径与策略可替换性。

### 6.1 非交互 deny-by-default（Phase 2 核心验收）

- E2E-P2-NI-001：非交互默认拒绝（无 allow-list）
  - 入口：`tool execute --state-file ...`
  - 输入：`{"command":"echo hi"}`
  - 期望：拒绝；分类码为 `approval_required` 或 `not_in_allow_list`（二选一但必须固定）
  - 审计：写入一条记录，decision 为 deny/require_approval（按策略固定）
- E2E-P2-NI-002：显式 allow-list 放行
  - 配置：`--shell-allow echo`
  - 输入：`echo hi`
  - 期望：允许执行；`exit_code==0`；audit 记录含 exit_code

### 6.2 空输入行为（Phase 2 验收项）

- E2E-P2-EMPTY-001：空字符串拒绝
  - 输入：`{"command":""}`
  - 期望：拒绝；分类码 `empty_command`；审计记录存在
- E2E-P2-EMPTY-002：仅空白拒绝
  - 输入：`{"command":"   "}`
  - 期望：同上

### 6.3 危险模式拒绝矩阵（Phase 2 验收项）

每条用例都要求：即使 allow-list 命中（例如允许 `echo`），仍必须拒绝危险模式。

- E2E-P2-DANGER-001：变量展开（裸 `$VAR` / `${VAR}`）拒绝
  - `echo $HOME`
  - `echo ${HOME}`
- E2E-P2-DANGER-002：命令替换拒绝
  - `echo $(whoami)`
  - ``echo `whoami````
- E2E-P2-DANGER-003：重定向拒绝
  - `echo hi > out.txt`
  - `cat < README.md`
- E2E-P2-DANGER-004：单独 `&` 拒绝
  - `echo hi &`
- E2E-P2-DANGER-005：控制字符拒绝
  - 包含 `\n` 或 `\r` 的命令串（通过 JSON 字符串注入）

期望：分类码统一为 `dangerous_pattern`（或更细分，但必须固定）；审计记录存在且不泄露敏感信息。

### 6.4 pipeline/compound operator 行为（Phase 2 验收项）

要求：按 `;`、`|`、`&&`、`||` 分段，逐段校验“程序名在 allow-list 内”，任一段不满足则整体拒绝。

- E2E-P2-SEG-001：`echo` + `wc` 都允许则 pipeline 允许
  - allow-list：`echo,wc`
  - command：`echo hi | wc -c`
  - 期望：允许执行；审计记录含 exit_code
- E2E-P2-SEG-002：仅允许 `echo` 则 pipeline 拒绝
  - allow-list：`echo`
  - command：`echo hi | wc -c`
  - 期望：拒绝；分类码 `not_in_allow_list`
- E2E-P2-SEG-003：`;` 序列中混入未允许命令则拒绝
  - allow-list：`echo`
  - command：`echo a; ls`
  - 期望：拒绝；分类码 `not_in_allow_list`
- E2E-P2-SEG-004：`&&` / `||` 行为一致（逐段校验）
  - allow-list：`echo,false,true`（或按平台可用命令替代，但应避免依赖外部环境差异）
  - command：`echo a && echo b`

注意：命令解析/切分/分词的“支持边界”必须在对外契约中写死，并用 E2E 回归锁定；若未来增强 quoting/转义语义，需要同步更新契约与用例，避免同一条用例在不同版本含义漂移。

### 6.5 allow-list 解析与配置优先级

- E2E-P2-ALLOW-001：`--shell-allow-file` 解析（建议新增）
  - 文件内容：空行、`# comment`、重复项、前后空白
  - 期望：解析稳定，去重稳定，空/注释忽略稳定
- E2E-P2-ALLOW-002：CLI flag 覆盖 env
  - env：`DEEPAGENTS_SHELL_ALLOW=ls`
  - flag：`--shell-allow echo`
  - command：`echo hi`
  - 期望：允许（证明 flag 生效）；同时 `ls` 不应被隐式允许

### 6.6 审计输出（拒绝与放行都要审计）

- E2E-P2-AUDIT-001：拒绝也写审计记录
  - 触发：E2E-P2-NI-001 / E2E-P2-DANGER-003
  - 断言：audit.jsonl 行数 +1；decision 与分类码一致；无 exit_code
- E2E-P2-AUDIT-002：放行写审计记录（含 exit_code 与 duration_ms）
  - 触发：E2E-P2-NI-002
  - 断言：exit_code 存在且为整数；duration_ms 存在且为整数

### 6.7 脱敏（不记录敏感信息）

- E2E-P2-REDACT-001：参数型 secret 脱敏
  - command：`echo --token abc123`
  - 断言：审计中出现 `--token ***`；不出现 `abc123`
- E2E-P2-REDACT-002：赋值型 secret 脱敏
  - command：`echo TOKEN=abc123`
  - 断言：审计中出现 `TOKEN=***`；不出现 `abc123`

### 6.8 run 路径不绕过策略（必须覆盖）

使用确定性的输入驱动 `run` 触发一次 `execute`（例如通过 mock provider 脚本或等价机制；具体驱动方式不属于 E2E 断言内容）。

- E2E-P2-RUN-001：run 模式下未允许 execute 必拒绝（非交互）
  - 配置：默认 non-interactive；不提供 allow-list
  - 期望：`tool_results[0].error` 可观测分类码；审计写入；`RunOutput.error` 的处理策略固定（可为 null 但 tool_results 有 error，或直接 runtime_error，但必须契约化）
- E2E-P2-RUN-002：run 模式下 allow-list 放行 execute
  - 配置：`--shell-allow echo`
  - 期望：execute 成功，tool_results.output.exit_code==0；审计写入

## 7. 迭代门禁（建议按里程碑推进 E2E）

对齐 Phase 2 详细计划的 M0~M3，本 E2E 建议分三轮门禁：

- I1（deny-by-default + 审计基线）
  - E2E-P2-NI-001/002
  - E2E-P2-AUDIT-001/002
  - E2E-P2-REDACT-001/002
- I2（危险模式 + pipeline/compound + 空输入）
  - E2E-P2-EMPTY-001/002
  - E2E-P2-DANGER-001~005
  - E2E-P2-SEG-001~004
- I3（run 路径不绕过 + 配置优先级）
  - E2E-P2-RUN-001/002
  - E2E-P2-ALLOW-001/002

## 8. 与 Python 用例集合对齐（测试数据的组织建议）

Phase 2 要求“对齐 Python 用例集合”，但不依赖 Python 代码细节。建议将用例向量以数据文件形式固化（Rust 侧读入跑表驱动测试）：

- `fixtures/command_policy_cases.json`
  - `command`
  - `allow_list`
  - `mode`
  - `expected_decision`
  - `expected_reason_code`

同一份 cases 可用于：

- CLI 集成/E2E（tool/run 入口）
-（可选）策略层单测（若项目选择为策略模块补齐单测，这不影响 E2E 的黑盒性质）

## 9. 黑盒测试边界（本计划的刻意取舍）

本计划只验证“对第三者可观察”的结果：退出码、stdout JSON、文件副作用、审计文件内容。

- `ApprovalPolicy` trait 属于可扩展点本身，除非 CLI 提供“选择/加载策略实现”的外部入口（例如 `--approval-policy ...` 或环境变量），否则其“可替换性”无法用纯黑盒 E2E 验证。
- 若 Phase 2 需要把“第三方策略可接入”纳入可执行验收，建议同步定义一个对外注入点，并新增 E2E：
  - 在同一套用例下切换不同策略实现，得到不同的 allow/deny/require_approval 决策与审计记录。
