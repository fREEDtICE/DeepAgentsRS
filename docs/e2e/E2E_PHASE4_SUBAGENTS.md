# Phase 4 E2E 测试计划（SubAgentMiddleware：task 工具与子代理路由，黑盒）

适用范围：本计划面向 [ITERATION_PLAN.md](../iteration/ITERATION_PLAN.md#L158-L177) 的 Phase 4：支持“Task tool / 子代理”的注册与调用，并提供子代理路由、权限边界、以及子代理结果的可控合并策略。**本计划为黑盒 E2E**：只关注外部可观察行为与结果，不依赖实现细节、内部模块、源码结构、线程模型或具体运行时选型。

目标基准：让“父代理/运行时”能够像调用普通工具一样调用 `task` 工具，`task` 工具能选择某个子代理并执行它的工作流（可能包含工具调用），最终把子代理结果以结构化形式返回，并满足“子代理不越权（根目录/命令权限），结果可控合并”的验收要求。

---

## 0. 术语与约束

- **父代理（Parent）**：发起 `task` 工具调用的一方（例如 `deepagents run ...` 的一次运行，或 ACP 会话内的一次调用）。
- **子代理（SubAgent）**：被 `task` 选中并执行的一方。子代理可能会调用工具（read/write/edit/grep/execute 等）。
- **task 工具（Task Tool）**：对外暴露的“子代理调用”入口。输入指定子代理类型与任务描述，输出子代理执行结果（含可选的 trace/state）。
- **Root**：文件系统 sandbox 的根目录边界。所有文件类工具必须受 root 约束。
- **执行权限（Execute Policy）**：命令执行 deny-by-default + allow-list/审批（Phase 2）。Phase 4 必须保证子代理不能绕过父侧策略。
- **隔离与合并（Isolation/Merge）**：子代理执行时的状态（state）与工具权限应隔离；子代理完成后，父侧对“哪些结果被合并回去”有明确、可回归的策略。

黑盒测试原则：

- 不假设子代理内部实现（例如是否复用同一 runtime、是否新进程、是否 async 并发）
- 只断言：输入/输出、权限边界、状态变化、错误码语义、时序与可重复性
- 若外部协议尚未最终确定，本计划允许“兼容断言”，但必须固化最小可测契约点，否则 E2E 无法落地

---

## 1. 完成定义（E2E 角度）

Phase 4 E2E 通过必须满足：

- 存在可脚本化的入口可触发 `task`（例如 `deepagents run ...` 或 ACP `call_tool`），且输出可稳定解析为结构化结果（JSON）。
- `task` 支持按“子代理类型/名称”路由到目标子代理，并能返回子代理的最终输出（final_text 或等价字段）。
- 子代理权限边界可回归：
  - 文件访问不越 root（包括 `../` 与符号链接绕行，若纳入 root 约束）
  - 命令执行不绕过父侧策略（deny-by-default、allow-list/审批）
  - 子代理可用工具集合受控（不能调用未授权工具）
- 子代理结果合并策略可回归：
  - 至少明确并测试：子代理的 state 是否合并、如何合并、冲突如何处理、删除语义如何表达
  - 父侧能观察到合并结果（通过最终 state 或 delta/state_version）
- 错误语义稳定：unknown subagent、schema 校验失败、子代理超时、子代理内部错误、子代理工具错误等都可分类且不导致宿主崩溃。

不范围（Phase 4 不要求）：

- 子代理“智能规划质量”（例如是否真的更聪明、更会搜）
- 真实 LLM 质量与提示词工程
- memory/summarization 等后续中间件能力（Phase 7/8）

---

## 2. 测试入口与 Harness（第三者视角）

本计划不强制具体协议，但要求能完成“触发 task 并拿到结构化输出”。建议优先选择仓库已存在/已承诺长期维护的产品形态入口，以保证 E2E 不是一次性样例。

### 2.1 推荐 Harness（优先级从高到低）

1) **CLI 黑盒（推荐）**  
以非交互方式运行一次父代理执行，stdout 输出单个 JSON（不混入日志），E2E 通过 spawn 子进程 + 解析 JSON + 断言语义。

2) **ACP 黑盒**  
启动 ACP server，client 通过网络接口调用 `call_tool`，对 `task` 工具做黑盒断言。

3) **纯库集成（不推荐作为主 E2E）**  
直接在测试里调用 Rust API 容易绑死实现细节，不符合“黑盒”目标；可作为补充，但不作为门禁主入口。

### 2.2 必须固化的最小外部契约点（否则 E2E 无法落地）

无论采用 CLI 还是 ACP，必须提供等价能力：

1) **调用 task 工具**  
输入：`subagent_type`（或 name）、`query`（或 task 描述）、以及可选的 `response_language`  
输出：结构化结果，至少包含：
- 子代理最终输出（如 `final_text` 或 `output.text`）
- 子代理错误（可空；失败时含错误码/原因）
- 子代理执行中产生的“可审计摘要”（至少能知道是哪个子代理被调用）

2) **可观测合并结果**  
至少满足其一：
- `task` 工具输出中直接携带 `state` 或 `delta`
- 父运行最终输出中可读到合并后的 `state`（或 `state_version` + 可查询 delta）

3) **可控权限策略**  
测试必须能通过外部方式配置：
- root（临时目录）
- execute allow-list/非交互策略（Phase 2 的对外配置）
- 子代理可用工具集合（至少可为“默认子代理”设定工具边界）

---

## 3. 对外契约建议（为 E2E 可测性而定义的最小稳定语义）

如果 Phase 4 尚未把 `task` 的输入输出 schema 完全固化，建议按以下最小契约落地（字段名可调整，但语义必须可等价映射，并在文档中给出映射关系）。

### 3.1 task 输入（TaskInput）

```json
{
  "description": "短描述（用于审计/trace）",
  "query": "子代理实际任务内容",
  "subagent_type": "search | code-reviewer | ...",
  "response_language": "zh-CN"
}
```

语义要求：

- `subagent_type` 必须是可枚举/可校验的标识符；不存在时返回 `subagent_not_found`
- `query` 必填；为空返回 `invalid_input`
- `response_language` 可选；缺省继承父侧默认

与 Python 语义对齐的建议（可观察行为优先）：

- 若目标是对齐 Python `task` 工具的“对模型可见 schema”，建议对模型侧仅暴露 `{description, subagent_type}` 两个字段；将本处 `query` 合并进 `description`（或将 `description` 作为对外审计字段而不暴露给模型）。
- `response_language`、`tool_call_id`、父/子 state、root 与 execute 策略等应由运行时注入，不进入 tool schema（避免被模型伪造与避免 schema 漂移）。

### 3.2 task 输出（TaskOutput）

```json
{
  "ok": true,
  "result": {
    "subagent_type": "search",
    "final_text": "....",
    "error": null,
    "state": { "filesystem": { "...": "..." } },
    "delta": null,
    "trace": { "terminated_reason": "final_text", "tool_calls": 2 }
  }
}
```

语义要求：

- `final_text`：子代理最终可读输出（成功时必须可用）
- `error`：失败时必须提供可分类 `code`（例如 `subagent_timeout`/`subagent_error`/`invalid_input` 等）
- `state/delta`：至少其一可用于验证合并策略是否生效
- `trace`：至少能知道调用了哪个子代理、执行是否超时、是否调用工具（数量级信息即可）

备注：如果顶层协议不是 `{ok,result,error}`，也允许，但必须能无歧义映射上述语义字段。

---

## 4. Phase 4 统一 Fixture（所有用例复用）

每个用例创建独立 root（临时目录），建议预置：

- `README.md`：包含一段可唯一识别文本 `needle_phase4`
- `src/lib.rs`：包含 `needle_phase4` 与其他随机词
- `empty.txt`：空文件
- `large.txt`：> 500 行（用于 read_file 截断/分页相关断言，如纳入）
- `secret.txt`：仅在 root 内的敏感文本（用于“不能越界泄露”断言）

root 外（临时目录的父目录或另建目录）预置：

- `outside_secret.txt`：root 外敏感文本（用于越界访问拒绝）

可选（若系统支持且纳入 root 约束）：

- `link_to_outside`：root 内符号链接，指向 root 外 `outside_secret.txt`（用于“符号链接绕行拒绝”）

---

## 5. E2E 用例清单（按能力域分组）

说明：

- 每条用例都只断言“必须稳定”的语义点，避免依赖字符串细节或内部结构。
- 错误码名称在下文以建议值给出；实现可调整，但需在文档中给出稳定枚举，并在 E2E 中严格断言。

### 5.1 子代理注册与可发现性（基础）

**E2E-SUB-REG-001：task 可调用已注册子代理**

- 前置：系统内至少存在一个可用子代理（例如 `search` 或 `mock_subagent`）
- 步骤：调用 task，指定该子代理
- 期望：成功返回 `final_text`
- 断言：输出中包含 `subagent_type` 且等于输入；`final_text` 非空

**E2E-SUB-REG-002：unknown subagent 拒绝**

- 步骤：调用 task，`subagent_type="does_not_exist"`
- 期望：失败但不崩溃
- 断言：错误码 `subagent_not_found`；父侧进程/会话仍可继续后续调用

**E2E-SUB-REG-003：list_subagents（若对外提供）**

- 步骤：调用“列出子代理”的接口（可选：工具或 API）
- 断言：返回子代理列表；每项包含 `name/type` 与 `description`（至少可读信息）

### 5.2 路由与最小闭环（核心）

**E2E-SUB-ROUTE-001：task 路由到子代理并返回结果**

- 输入：`subagent_type=<A>`，`query="返回固定短语 ping"`（或等价可预测任务）
- 期望：子代理执行并返回
- 断言：`final_text` 包含 `ping`（或包含可预测标记）

**E2E-SUB-ROUTE-002：同一父运行内连续调用两次 task**

- 步骤：在一次父运行（或同一 ACP session）内调用 task 两次（不同 query）
- 断言：两次结果都可用；trace 中可区分两次调用（例如不同 call_id/run_id）

**E2E-SUB-ROUTE-003：子代理可调用工具并影响其输出**

- 前置：子代理具备至少一个只读工具能力（read_file 或 grep）
- 步骤：task query 要求“读取 README.md 第一行并返回”
- 断言：`final_text` 包含 README 第一行（或包含 `needle_phase4`）

**E2E-SUB-ISO-001：父侧只接收子代理最终输出（不合并全对话）**

- 前置：提供一个确定性子代理（如 `mock_subagent`），其执行过程中会产生多条中间输出（多轮 tool call 或多轮推理）
- 步骤：调用 task，并让子代理产生多轮过程后返回最终结论
- 断言：父侧可观测的 `task` 工具输出只包含子代理最终可读输出（`final_text` 或等价字段）；不会把子代理的完整 messages/history 全量合并回父侧（避免信息泄露与 token 膨胀）

### 5.3 权限边界：文件系统 Root（必须）

**E2E-SUB-SEC-FS-001：子代理不可越界读取 ../**

- 步骤：task query 要求读取 `../outside_secret.txt`
- 期望：拒绝
- 断言：错误码 `invalid_path` 或 `permission_denied`；输出不包含 outside_secret 内容

**E2E-SUB-SEC-FS-002：子代理不可通过符号链接越界（若纳入）**

- 步骤：task query 要求读取 `link_to_outside`
- 期望：拒绝
- 断言：错误码稳定；输出不包含 outside_secret 内容

**E2E-SUB-SEC-FS-003：子代理读取 root 内 secret 不应泄露到错误信息**

- 步骤：让子代理触发一个失败（例如读取不存在文件），同时 root 内存在 `secret.txt`
- 断言：错误信息不包含 `secret.txt` 内容（最多包含路径名/错误码）

### 5.4 权限边界：execute 策略继承与不可绕过（必须）

**E2E-SUB-SEC-EXEC-001：父侧 deny-by-default 时子代理 execute 被拒绝**

- 前置：父侧未配置 allow-list（或明确 non-interactive deny-by-default）
- 步骤：task query 要求执行 `echo hello`
- 期望：拒绝且不执行
- 断言：错误码 `command_not_allowed` 或 `approval_required`；不会产生任何可见副作用文件

**E2E-SUB-SEC-EXEC-002：父侧 allow-list 放行后子代理 execute 可用**

- 前置：父侧配置 allow-list 允许 `echo`（或测试专用安全命令）
- 步骤：task query 要求执行 `echo hello`
- 断言：exit_code=0（或等价成功信号）；输出包含 hello

**E2E-SUB-SEC-EXEC-003：子代理不能扩大父侧 allow-list**

- 步骤：父侧 allow-list 仅允许 `echo`，task query 要求执行未允许命令（例如 `uname` 或 `cat`）
- 断言：拒绝；错误码稳定；不执行

### 5.5 权限边界：子代理工具集合（必须）

**E2E-SUB-SEC-TOOLS-001：子代理只能使用其被授予的工具**

- 前置：存在一个“只读子代理”（例如只允许 read_file/grep，不允许 write/edit/delete/execute）
- 步骤：task query 要求该子代理写文件 `a.txt`
- 期望：拒绝
- 断言：错误码 `tool_not_allowed`（或等价）；root 中不存在 `a.txt`

**E2E-SUB-SEC-TOOLS-002：子代理无法调用未注册工具**

- 步骤：task query 引导子代理调用不存在工具名（例如 `__no_such_tool__`）
- 断言：错误码 `unknown_tool`；父侧仍可继续运行并得到可诊断结果

### 5.6 状态隔离与合并策略（核心）

本组用例的目的不是规定“必须怎么做”，而是强制把策略变成可回归的外部行为。

建议 Phase 4 对外提供一种可配置策略（至少在测试环境可配置）：

- `merge_state: "none" | "delta" | "full"`（示例枚举）
- `merge_messages: boolean`（是否把子代理对话/中间输出合并到父侧可见历史）
- `merge_tool_traces: boolean`（是否合并子代理工具调用记录）

**E2E-SUB-MERGE-001：默认隔离（不自动合并写操作）**

- 前置：默认策略为“安全优先”（建议默认 `merge_state=delta` 或 `none`，但必须文档化并在此用例中锁定默认行为）
- 步骤：task 让子代理写 `a.txt` 并返回“写入成功”
- 断言（按默认策略固定其一）：
  - 若默认不合并：父侧最终 state 不包含 `a.txt`（或 delta 不包含）
  - 若默认合并：父侧最终 state 包含 `a.txt` 且内容符合预期

**E2E-SUB-MERGE-002：显式开启合并后写操作可回填**

- 前置：以外部方式开启合并（例如 CLI flag / session option）
- 步骤：子代理 write_file `a.txt` 内容为 `hello`
- 断言：父侧最终 state（或 delta）出现 `a.txt`，且内容为 `hello`（或符合截断契约）

**E2E-SUB-MERGE-003：冲突合并语义可回归**

- 步骤：
  1) 父侧先写 `a.txt` 为 `parent`
  2) 调用 task，让子代理把 `a.txt` 改为 `child`
  3) 读取父侧最终 state
- 断言：冲突策略固定并可回归（选择其一并文档化）：
  - 子代理覆盖父侧（last-write-wins）
  - 父侧优先（child changes dropped）
  - 合并失败并返回 `merge_conflict`

**E2E-SUB-MERGE-004：删除语义可回归**

- 步骤：父侧创建 `b.txt`，task 让子代理删除 `b.txt`
- 断言：父侧 state 中 `b.txt` 被移除或被标记 deleted=true（二选一并固化）

### 5.7 错误传播与鲁棒性（必须）

**E2E-SUB-ERR-001：子代理内部错误被分类并返回**

- 步骤：触发子代理自身错误（例如请求它做“必失败”的动作：调用不存在工具）
- 断言：错误码为 `subagent_error` 或更细分（如 `subagent_tool_error`）；父侧不会崩溃

**E2E-SUB-ERR-002：task 输入 schema 校验严格**

- 步骤：构造非法 task 输入（缺 `query`、`subagent_type` 类型错误、额外字段如 strict）
- 断言：返回 `invalid_input`/`schema_validation_failed`；不会 fallback 到默认子代理；不会执行任何工具副作用

**E2E-SUB-ERR-003：子代理超时**

- 前置：父侧或 task 可配置 `subagent_timeout_ms`
- 步骤：让子代理执行一个可控长任务（例如 sleep 或大量 grep；不依赖真实网络）
- 断言：错误码 `subagent_timeout`；超时后资源被回收（同一会话可继续调用 task）

**E2E-SUB-ERR-004：子代理 step 上限（若适用）**

- 前置：子代理有 `max_steps` 或等价限制
- 步骤：构造使其无法终止的任务（或测试专用 mock 子代理）
- 断言：错误码 `subagent_max_steps_exceeded`；返回可诊断 trace

### 5.8 多任务并发与隔离（建议门禁）

**E2E-SUB-CONC-001：并发调用两个不同子代理不会串台**

- 步骤：并发触发 task(A) 与 task(B)，分别读取不同文件并返回不同标记
- 断言：结果与各自 query 对应；trace/subagent_type 不混淆

**E2E-SUB-CONC-002：并发写入的合并结果符合既定策略**

- 前置：开启合并
- 步骤：并发 task 写同一文件不同内容
- 断言：最终结果符合冲突策略（例如 last-write-wins，但必须稳定且可解释）

### 5.9 可审计与信息泄露（必须）

**E2E-SUB-AUDIT-001：task 输出可审计但不泄露敏感内容**

- 步骤：在 root 内准备 `secret.txt`，让子代理执行失败并返回错误
- 断言：trace/错误信息中不包含 secret.txt 内容；只包含必要的错误码与路径（若需要）

**E2E-SUB-AUDIT-002：父侧可识别子代理调用链**

- 步骤：一次运行内调用 task（至少一次）
- 断言：父侧输出中存在可追踪字段（例如 `tool_calls` 中含 task 调用记录，或 trace 中含 subagent_run_id）

---

## 6. 结果断言规范（黑盒一致性）

为确保后续协议扩展（CLI/ACP/其他传输）仍可复用测试，建议 E2E 统一断言以下语义：

- 成功时必须能取到子代理“最终可读输出”（final_text 或等价）
- 失败时必须能取到可分类错误码（string enum），且不会用“任意字符串”替代
- 子代理不越权：root 越界读与 execute 绕过必须在 E2E 中硬性失败
- 合并策略外显：父侧最终 state/delta 必须可用来验证是否合并、如何合并

---

## 7. 迭代门禁建议（Phase 4）

建议分三道门禁，避免一次性堆满用例导致定位困难：

- I1（闭环基线）：REG-001、ROUTE-001、ROUTE-003、ERR-002、AUDIT-002
- I2（安全边界）：SEC-FS-001、SEC-EXEC-001、SEC-EXEC-003、SEC-TOOLS-001、AUDIT-001
- I3（合并可控 + 鲁棒）：MERGE-001/002/003/004、ERR-003、CONC-001（可选 CONC-002）

---

## 8. 落地建议（从计划到可执行）

- 测试工程建议（二选一）：
  - CLI 黑盒：`crates/deepagents-cli/tests/e2e_phase4_subagents.rs`（spawn 二进制、解析 stdout JSON）
  - ACP 黑盒：`crates/deepagents-acp/tests/e2e_phase4_subagents_acp.rs`（启动 server、网络请求）
- 为确保用例可重复，强烈建议提供“确定性子代理”用于 E2E（例如 `mock_subagent` 或脚本驱动子代理），以避免真实模型/非确定性推理导致 flake。
- 若子代理需要网络搜索等能力，E2E 中应使用离线可控数据源（fixture 文件或本地 index），禁止依赖外部互联网。
