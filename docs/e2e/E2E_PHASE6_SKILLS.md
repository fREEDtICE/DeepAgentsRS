# Phase 6 E2E 测试计划（SkillsMiddleware：技能加载与校验，黑盒）

适用范围：本计划面向 [ITERATION_PLAN.md](../iteration/ITERATION_PLAN.md#L206-L227) 的 Phase 6：实现“技能生态”的产品化闭环（多 source 发现与覆盖、技能包校验、安全边界、对模型可见注入、执行期隔离、DevX）。本计划为黑盒 E2E：只关注外部可观察行为与结果，不依赖源码结构、内部模块、序列化库、线程模型或中间件实现细节。

对齐锚点（行为优先）：

- Extras 验收索引（按生命周期划分的必测能力）：[skills/index.md](../acceptance_extras/skills/index.md)
- Phase 6 详细契约与范围定义：[ITERATION_PHASE6_DETAILED.md](../iteration/ITERATION_PHASE6_DETAILED.md)

---

## 0. 背景与测试目标（黑盒视角）

技能生态的端到端目标是：用户只通过配置 skills 源路径（sources）即可获得新增工具能力，且这些能力：

- 可发现：能列出“最终生效技能清单”，并能诊断每个技能来自哪个 source、是否发生覆盖。
- 可控：技能执行受限于 deny-by-default 的权限与资源策略，不可越权（尤其是 filesystem/execute）。
- 可观测：模型侧能“看到”技能（tools/schema + system 注入），并能在 run 输出中回溯“调用了哪个技能、结果如何、为何失败”。
- 可维护：开发者可以初始化一个最小技能模板并在 CI 中快速校验（init/validate）。

黑盒原则：

- 不依赖具体实现路径（技能是“宏工具”还是“skill_call 展开”都可），但必须提供等价的外部观察点以断言上述效果。
- 不依赖真实 LLM：必须使用确定性模型（ScriptedModel/MockProvider）驱动技能调用，避免 flake。
- 不依赖外部网络：禁止访问公网；如需“网络能力”只测试权限拒绝，不测试真实网络请求。

---

## 1. 术语与外部可测对象

- **skills source**：一个目录路径。其下每个子目录代表一个 skill 包。
- **skill 包**：一个目录，至少包含 `SKILL.md`（含 YAML frontmatter 元数据）。可选包含 `tools.json`（可执行工具定义）。缺 `tools.json` 的 skill 视为 prompt-only（不注册可调用工具）。
- **skill tool**：由技能包声明/生成的可调用工具（有 name/description/input_schema）。
- **注入（injection）**：让模型在每轮调用前能“看到技能”的信息注入，至少包含：
  - tools/schema 注入（模型可调用）
  - system prompt 注入（模型可读说明/约束）
- **执行期隔离（isolation）**：技能执行出错/超时/权限不足不能拖垮 runner；技能不能绕过全局执行策略（Phase 2）。

---

## 2. 完成定义（E2E 角度）

Phase 6 E2E 通过必须满足：

- 存在可脚本化入口可以：
  - 发现/列出/校验 skills（list/validate）
  - 在一次运行中启用 skills 并触发技能调用（run）
  - 初始化技能模板（init）
- “多 source + last one wins” 覆盖规则可回归（SD-01~SD-04）。
- 注入可回归：
  - tools/schema 层：模型能在 request 中看到技能工具（或等价可测表示）（SL-01）
  - system 层：system prompt 中出现技能说明块（可快照断言）（SL-02）
  - 幂等：重复 run/resume 不应重复注入同一块（marker 或等价机制）（SL-02 延伸）
- 执行与隔离可回归（SEI-01~SEI-05）：
  - deny-by-default 权限（filesystem/execute/network）
  - 超时取消
  - 大输出处理策略固定（截断或 offload 二选一，Phase 6 默认推荐截断）
  - panic/异常不崩溃
  - 不能绕过全局 execute 策略（allow-list/非交互 deny-by-default/审计）
- DevX 可回归（SDX-01~SDX-03）：init 产物可被 validate；validate 在错误时非 0 且可定位；validate 成功可用于 CI。

---

## 3. 测试入口与 Harness（第三者视角）

本计划推荐以 CLI 作为主黑盒入口（ACP 作为补充入口）。任何入口都必须满足“可脚本化 + stdout 结构化 + 退出码稳定”。

### 3.1 CLI 入口（推荐门禁）

要求提供等价能力（命令名可不同，但语义必须映射清晰并文档化）：

- `deepagents skill list --source <dir>... [--pretty]`
  - 输出：结构化 JSON，至少包含 `skills`、`tools`、`diagnostics`（或能等价映射）。
- `deepagents skill validate --source <dir>... [--pretty]`
  - 只做加载+校验，不启动完整 runner。
  - 退出码：成功 0；失败非 0；stdout/错误信息必须可定位到 skill/文件/字段。
- `deepagents skill init <dir>`
  - 生成最小技能模板（至少 `SKILL.md`，以及可选的 `tools.json` 示例）。
- `deepagents run --skills-source <dir>... --provider <scripted> --mock-script <path> ...`
  - 用确定性模型脚本触发技能工具调用，并在 stdout 输出单个 JSON（RunOutput 或等价结构）。

stdout/stderr 基线（为 E2E 稳定性强制）：

- stdout 只输出单个 JSON 对象（不混日志）。
- stderr 可输出日志，但 E2E 断言不得依赖 stderr 文案（仅允许在“必须定位错误”场景做包含性断言）。

### 3.2 确定性模型（必须）

E2E 必须使用“脚本驱动模型”，确保每次都触发同一组 tool_calls：

- Step 1：输出对某个技能工具的 tool_call（或 skill_call，视外部协议而定）
- Step 2：输出 final_text（或从 last_tool_result 派生 final_text），用于验证“技能结果确实进入了对话闭环”

核心要求是：测试能断言 runner 发给模型的 request 中包含“技能工具/schema”和“system 注入块”，且能稳定触发技能工具执行。

---

## 4. Phase 6 统一 Fixture（技能资产库 + 工作区 root）

建议新增/沉淀可复用的 fixture 目录（路径可调整，但结构建议固定）：

- `fixtures/skills/sources/A/`：source A（用于覆盖矩阵）
- `fixtures/skills/sources/B/`：source B（用于覆盖矩阵）
- `fixtures/skills/invalid/`：非法技能包集合（每个子目录一个 case）
- `fixtures/skills/mock_scripts/`：确定性模型脚本集合
- `fixtures/workspace_root/`：被技能读写的工作区模板（每个用例复制到临时目录）

### 4.1 workspace root 模板（每用例隔离）

每个用例创建独立临时 root（避免相互污染），预置：

- `README.md`：首行包含唯一标记，例如 `needle_phase6`，用于“read_file→final_text”断言。
- `safe.txt`：普通文件。
- `secret.txt`：敏感内容（用于验证“不泄露到错误信息/审计/trace”）。
- `out/`：空目录（用于写入副作用断言）。

root 外预置（用于越界/绕行测试）：

- `outside_secret.txt`：root 外敏感内容。
- 可选：root 内创建符号链接 `link_to_outside` 指向 root 外文件（用于“符号链接绕行”断言；若系统选择“允许 symlink 但限制越界”，则必须固化相应规则）。

### 4.2 skill 包模板（正向）

建议至少提供下列技能（名称只是示例，关键是语义可区分、可断言）：

- `math-add/`：纯计算，无副作用；输入 `a,b`，输出 `a+b`（用于 SL/SEI 基础闭环）。
- `echo-skill/`：输入 `text`，输出 `"E:"+text`（用于参数传递与 schema 必填验证）。
- `fs-skill/`：尝试写文件（例如写入 `out/skill.txt`），用于 SEI-01 权限与副作用断言。
- `exec-skill/`：尝试执行 `echo 1`，用于 SEI-02 与 Phase 2 策略叠加断言。
- `long-skill/`：可控地“很慢”（用于 SEI-03 超时）。
- `big-skill/`：返回超大字符串（用于 SEI-04 大输出策略）。
- `panic-skill/`：可控触发异常（用于 SEI-05）。
- `prompt-only/`：仅 `SKILL.md` 无 `tools.json`，用于“只注入不注册工具”的边界断言。

### 4.3 skill 包模板（反向/非法）

每个非法 case 独立目录，便于定位：

- 缺 `SKILL.md`
- `SKILL.md` 无 frontmatter 或 frontmatter 不闭合
- frontmatter 字段未知/类型错误
- name 与目录名不一致/包含非法字符/过长/过短
- `tools.json` 非法 JSON
- `tools.json` 含未知字段（strict deny unknown fields）
- tool 名称与 core tools 冲突（如 `read_file`）
- steps 数量超过 policy.max_steps（或超过系统上限）
- input_schema 非 object/required 与 properties 不一致
- 技能目录或关键文件为 symlink（应拒绝或固定策略）

---

## 5. 结果断言规范（黑盒一致性）

为抵抗实现细节演进，E2E 只断言“必须稳定”的语义字段。

### 5.1 list/validate 输出（结构化 + 可定位）

最小断言集合（字段名可变，但必须能映射）：

- `skills[]`：每项至少包含 `name`、`description`、`source`（或 path）。
- `tools[]`：每项至少包含 `name`、`description`、`input_schema`（或 schema 摘要）、`skill_name`（可选但强建议）。
- `diagnostics`：至少可表达：
  - sources 中哪些被跳过/失败（若支持 skip_invalid）
  - 覆盖发生的记录（overrides）
  - 每个 skill 的错误列表（文件路径/字段路径/错误码或类别）

退出码契约：

- validate 成功：exit 0
- validate 失败：exit 非 0，且输出/错误信息包含可定位信息（skill 名 + 文件名 + 字段路径或近似定位）

### 5.2 run 输出（闭环 + 注入可观测）

要求 run 输出的 JSON（RunOutput 或等价）至少可支持以下断言：

- 能找到“技能注入已发生”的证据（例如 system message 含固定 marker/固定前缀，或 trace 中有 `skills_injected=true`）。
- 能找到“技能工具可调用”的证据（例如 model tools 列表包含技能工具名；或 request.snapshot 中可读到 tools；或 trace 中记录已注入 tools 数量）。
- 能找到“技能工具被调用并执行”的证据：
  - tool_calls 中出现技能工具名（若技能工具化）
  - 或者出现 `skill_call` 记录，并能在 tool_results/trace 中看到其展开与执行结果
- 能断言“技能失败的错误码分类”：
  - schema 校验失败（invalid_request / schema_validation_failed）
  - 权限不足（permission_denied / tool_not_allowed）
  - 超时（skill_timeout）
  - 异常（skill_panic 或等价）

---

## 6. E2E 用例清单（按验收编号分组）

说明：

- 下文用例按 SD/SL/SEI/SDX 编号组织，确保与 Extras 验收一一对应。
- “命令示例”以 CLI 为主；如实际入口不同，必须在实现文档中给出等价映射。
- 每条用例都应具备：输入（source/root/script）、步骤（命令）、断言（stdout JSON 片段 + 退出码 + 文件副作用）。

### 6.1 发现与覆盖（SD-*）

**E2E-SK-SD-01：单 source 加载**

- 输入：`sources=[fixtures/skills/sources/A]`
- 步骤：`deepagents skill list --source A`
- 断言：
  - skills 列表包含 `web-research`（或等价示例 skill）
  - 该 skill 的 `source` 指向 A

**E2E-SK-SD-02：多 source 覆盖（last one wins）**

- 输入：`sources=[A,B]`，A 与 B 都包含同名 skill（描述或实现可区分）
- 步骤：`deepagents skill list --source A --source B`
- 断言：
  - 最终生效技能列表中同名 skill 只出现一次
  - 生效版本的 `source` 为 B
  - diagnostics 中存在覆盖记录（overrides），可定位“被覆盖版本→覆盖版本”

**E2E-SK-SD-03：覆盖后旧版本不可被调用**

- 输入：`sources=[A,B]`，A/B 的同名 skill 工具输出带可区分标记（例如 `A_IMPL` vs `B_IMPL`）
- 步骤：
  - 运行 `deepagents run --skills-source A --skills-source B --mock-script <calls_the_skill>`
- 断言：
  - 最终 tool_result 或 final_text 中只出现 B 的标记
  - 不可能出现 A 的标记

**E2E-SK-SD-04：无效 source 的错误语义固定**

- 输入：`sources=[/path/not_exist]`
- 步骤：
  - `deepagents skill validate --source /path/not_exist`
  - `deepagents run --skills-source /path/not_exist ...`（若 run 支持）
- 断言（二选一，必须固化并在实现文档中明确默认值）：
  - 方案 A（推荐默认）：启动/校验失败（exit 非 0），错误可定位为 invalid_source
  - 方案 B：跳过该 source（exit 0 或可继续 run），diagnostics 明确记录 skipped 与原因

### 6.2 加载与注入（SL-*）

**E2E-SK-SL-01：skills 工具出现在模型 tools/schema 中**

- 输入：提供至少两个 skill tool（如 `math-add` 与 `echo-skill`）
- 步骤：用确定性模型运行一次 `deepagents run --skills-source <src> --mock-script <inspect_request_then_call>`
- 断言：
  - 模型 request 的 tools 集合包含 `math-add` 与 `echo-skill`（或等价可观测表征）
  - `math-add` 的 schema 可断言 `a/b` 为必填，且类型匹配

**E2E-SK-SL-02：system prompt 注入包含技能说明块（且幂等）**

- 输入：任意含 2 个技能的 source
- 步骤：
  - 单次 run：`deepagents run --skills-source <src> --mock-script <no_call>`
  - 同一份会话/历史再次 run（或同一 run 的多轮 provider.step，若系统每轮都会构造 request）
- 断言：
  - system 中出现技能说明块（例如固定标题 `## Skills` 或固定 marker）
  - 注入不重复：同一 marker 只出现一次（或同一块不会重复追加）

**E2E-SK-SL-03：技能可被模型调用并回注**

- 输入：`math-add` 技能工具；脚本第 1 步发起 tool_call `math-add(a=1,b=2)`，第 2 步输出 final（可从 last_tool_result 派生）
- 步骤：`deepagents run --skills-source <src> --mock-script <call_math_add_then_finalize>`
- 断言：
  - 发生技能调用记录（tool_calls 或 skill_calls）
  - 有对应的 tool_result 且可被关联（call_id 对齐或等价关联）
  - final_text 受技能输出影响（包含 `3` 或等价结构）

**E2E-SK-SL-04：技能 schema 错误的可诊断失败（运行时路径）**

- 输入：一个 skill tool 的 input_schema 声明必填字段 `x`，但脚本发起调用时缺 `x`
- 步骤：`deepagents run --skills-source <src_with_bad_call> --mock-script <call_missing_required>`
- 断言：
  - tool_result/error 可分类为 schema/invalid_request
  - 错误信息能定位缺失字段名（例如包含 `missing required field: x`）
  - runner 不崩溃，后续 step 仍可继续并产生 final_text（或明确终止为可分类错误）

**E2E-SK-SL-05：skills 与 core tools 命名冲突处理**

- 输入：某 skill tool 名称与 core tool 冲突（例如 `read_file`）
- 步骤：`deepagents skill validate --source <src_conflict>` 或 `deepagents run --skills-source <src_conflict> ...`
- 断言（二选一，必须固化并文档化）：
  - 方案 A（推荐默认）：禁止冲突，validate/run 失败（exit 非 0），错误可定位为 tool_conflict_with_core
  - 方案 B：允许覆盖但必须显式开启（例如 allow_override=true），且 list/diagnostics 可清晰指出覆盖关系与最终生效工具

### 6.3 执行与隔离（SEI-*）

**E2E-SK-SEI-01：无权限时禁止 filesystem 副作用**

- 输入：skill `fs-skill` 尝试写 `out/skill.txt`；其 policy 或全局配置为 `allow_filesystem=false`
- 步骤：run 脚本调用 `fs-skill`
- 断言：
  - 返回权限不足错误（permission_denied/tool_not_allowed）
  - root 中不存在 `out/skill.txt`（强制检查文件副作用）

**E2E-SK-SEI-02：无权限时禁止 execute（且不能绕过 Phase 2）**

- 输入：skill `exec-skill` 尝试执行 `echo 1`；skill policy `allow_execute=false`
- 步骤：run 脚本调用 `exec-skill`
- 断言：
  - 直接被权限拒绝（不进入全局审批也可，但必须拒绝）
  - 不产生任何命令副作用/审计记录（若审计开启，记录也必须体现为拒绝而非执行）

扩展断言（叠加 Phase 2，全局策略必须生效）：

- 将 skill policy 改为 `allow_execute=true`，但 CLI 仍为非交互 deny-by-default 且未配置 allow-list
- 断言：仍被拒绝（command_not_allowed/approval_required），证明技能不能绕过全局策略

**E2E-SK-SEI-03：超时与取消（runner 不崩溃）**

- 输入：`long-skill` 执行时间可控地 > timeout；配置技能 timeout=1s（或非常小）
- 步骤：run 脚本调用 `long-skill`
- 断言：
  - 约定时间后返回超时错误（skill_timeout）
  - runner 不崩溃，后续脚本 step 仍可继续（或 run 以可分类错误终止，但进程必须正常输出结构化错误）

**E2E-SK-SEI-04：大输出处理策略固定（截断或 offload）**

- 输入：`big-skill` 返回超大字符串；配置 max_output_chars 很小
- 步骤：run 脚本调用 `big-skill`
- 断言（二选一，必须固化）：
  - 方案 A：输出截断并包含可断言标志（如 `truncated=true` 或 `...`），且不导致 stdout JSON 超大
  - 方案 B：输出 offload 到文件并返回“文件引用 + 头尾预览”结构（若选择该方案必须同时冻结路径模型与落盘位置）

**E2E-SK-SEI-05：panic/异常被转为可诊断错误（不崩溃）**

- 输入：`panic-skill` 可控触发异常
- 步骤：run 脚本调用 `panic-skill`
- 断言：
  - tool_result/error 可分类为 `skill_panic`（或等价）
  - runner 进程不崩溃，stdout 仍输出结构化 JSON（或输出结构化错误）

### 6.4 DevX（SDX-*）

**E2E-SK-SDX-01：skill init 生成的模板可被加载/校验**

- 步骤：
  - `deepagents skill init <tmp_skill_dir>`
  - `deepagents skill validate --source <tmp_skill_dir_parent>`
- 断言：
  - validate exit 0
  - 输出包含该 skill 的 name/description（以及 tools 摘要，如果模板包含 tools.json）

**E2E-SK-SDX-02：validate 能发现 schema/实现错误（且可定位）**

- 输入：对 init 生成的技能做一次“人为破坏”（例如 tools.json 引入未知字段、或 frontmatter 缺 name）
- 步骤：`deepagents skill validate --source <tmp_parent>`
- 断言：
  - exit 非 0
  - 输出/错误信息包含 skill 名与文件名（至少 `SKILL.md` 或 `tools.json`）以及可定位字段路径（近似定位也可，但需稳定）

**E2E-SK-SDX-03：validate 成功时可用于 CI**

- 步骤：在 fixtures 中准备一组“应通过”的 sources，运行 `deepagents skill validate --source <src1> --source <src2> --pretty`
- 断言：
  - exit 0
  - stdout JSON 可解析，包含 skills/tools 摘要（用于 CI 产物留档）

---

## 7. 迭代门禁建议（Phase 6）

建议按三道门禁逐步收敛，避免一次性堆满用例导致定位困难：

- I1（闭环基线）：SD-01、SD-02、SL-01、SL-02、SL-03、SDX-01、SDX-03
- I2（安全与隔离）：SEI-01、SEI-02（含“不能绕过全局 execute 策略”）、SEI-03、SEI-05、SL-05
- I3（鲁棒性与可维护）：SD-03、SD-04、SL-04、SEI-04、SDX-02

---

## 8. 落地建议（从计划到可执行）

- 测试工程建议（CLI 黑盒）：
  - `crates/deepagents-cli/tests/e2e_phase6_skills.rs`：spawn 二进制、解析 stdout JSON、断言语义与副作用。
- Fixture 沉淀建议：
  - 将 sources A/B、invalid cases、mock scripts 纳入仓库 fixtures，确保覆盖规则/错误定位稳定。
- Flake 控制：
  - 禁止真实网络；所有时间相关断言用“超时分类 + 不崩溃”而非精确耗时。
  - 每用例独立 temp root；用例结束清理；避免共享环境变量污染。
- 诊断与可审计：
  - 对于失败用例，强制保留 validate/list 输出 JSON 作为 CI artifact（便于排障）。
  - 审计（若启用）必须脱敏，不得包含 `secret.txt` 文件内容；E2E 可加入“错误信息/审计不泄露敏感内容”的附加断言（建议在 I2 纳入）。

---

## 附录 A：建议固化的错误码/诊断类别（用于黑盒断言）

本附录不强制字段名，但强制“可分类”。建议在 JSON 输出中选择一个稳定位置承载（例如 `error.code` 或 `tool_results[i].error.code`），并在 E2E 中严格断言枚举值。

- source/加载阶段
  - `invalid_source`：source 路径不存在/不可读/非目录
  - `skill_validation_failed`：技能包结构或元数据不合法（需要附带可定位信息）
  - `tool_conflict_with_core`：技能 tool 与 core tool 冲突（默认拒绝）
  - `tool_conflict_between_skills`：不同技能 tool 重名导致冲突（若选择拒绝而不是 last-one-wins）
- 注入阶段
  - `skills_injection_failed`：注入失败（应极少发生；必须包含原因）
- 执行阶段
  - `schema_validation_failed`：入参缺失/类型不匹配等
  - `permission_denied`：技能 policy 禁止某类能力（filesystem/execute/network）
  - `command_not_allowed` / `approval_required`：全局 execute 策略拒绝或要求审批
  - `skill_timeout`：技能工具整体超时
  - `skill_panic`：技能执行异常/崩溃被捕获并转为错误
  - `skill_steps_exceeded`：技能 steps 超过上限（若支持）
- 可选：面向 DevX
  - `template_write_failed`：init 写入失败（权限/路径问题）

---

## 附录 B：可执行用例矩阵（CLI 黑盒）

下表是“从文档到可执行测试”的一一映射模板。实现时允许调整命令行参数名与 JSON 字段名，但必须保持每条用例可用外部行为断言通过/失败。

表格中 JSON 断言使用“语义 JSONPath”描述（示例：`$.skills[?(@.name=="math-add")]`），实现时可用任意 JSON 解析方式完成等价断言。

| 用例 ID | 入口 | 命令示例（占位符） | 主要输入 | 退出码 | 必须断言（stdout JSON） | 必须断言（副作用） |
|---|---|---|---|---:|---|---|
| E2E-SK-SD-01 | list | `deepagents skill list --source <A>` | source=A | 0 | skills 包含 name=web-research 且 source=A | 无 |
| E2E-SK-SD-02 | list | `deepagents skill list --source <A> --source <B>` | sources=A,B | 0 | skills 中同名仅 1 个，source=B；diagnostics.overrides 非空 | 无 |
| E2E-SK-SD-03 | run | `deepagents run --root <tmp_root> --skills-source <A> --skills-source <B> --mock-script <s>` | sources=A,B；script=call_skill | 0 | final/tool_result 仅包含 B_IMPL | 无（或仅限允许的读） |
| E2E-SK-SD-04 | validate | `deepagents skill validate --source <NOT_EXIST>` | source=不存在 | 非 0（或 0，取决于策略） | A：error.code=invalid_source；B：diagnostics.sources[*].skipped=true | 无 |
| E2E-SK-SL-01 | run | `deepagents run ... --skills-source <src> --mock-script <inspect_then_call>` | script=inspect request | 0 | request.tools（或等价快照）包含 math-add/echo-skill；schema.required 含 a,b | 无 |
| E2E-SK-SL-02 | run | `deepagents run ... --mock-script <no_call>` | script=no_call | 0 | system 含 skills 块/marker；marker 仅 1 次 | 无 |
| E2E-SK-SL-03 | run | `deepagents run ... --mock-script <call_math_add>` | call math-add | 0 | tool_call/tool_result 可关联；输出为 3；final_text 引用结果 | 无 |
| E2E-SK-SL-04 | run | `deepagents run ... --mock-script <call_missing_required>` | 缺必填 | 0 或 非 0（需固化） | tool_result.error.code=schema_validation_failed；message 可定位缺失字段 | 无 |
| E2E-SK-SL-05 | validate | `deepagents skill validate --source <src_conflict>` | tool 名冲突 | 非 0（默认） | error.code=tool_conflict_with_core；定位到冲突 tool 名 | 无 |
| E2E-SK-SEI-01 | run | `deepagents run ... --mock-script <call_fs_skill>` | fs-skill；allow_filesystem=false | 0 或 非 0（需固化） | tool_result.error.code=permission_denied | root 中不存在 out/skill.txt |
| E2E-SK-SEI-02 | run | `deepagents run ... --mock-script <call_exec_skill>` | exec-skill；allow_execute=false | 0 或 非 0（需固化） | tool_result.error.code=permission_denied | 无命令副作用；若审计开启，只能记录拒绝 |
| E2E-SK-SEI-02B | run | `deepagents run --execution-mode non-interactive ...` | exec-skill；allow_execute=true；全局 deny | 0 或 非 0（需固化） | tool_result.error.code=command_not_allowed/approval_required | 无命令副作用 |
| E2E-SK-SEI-03 | run | `deepagents run ... --mock-script <call_long_skill>` | long-skill；timeout=1s | 0 或 非 0（需固化） | tool_result.error.code=skill_timeout | 无 |
| E2E-SK-SEI-04 | run | `deepagents run ... --mock-script <call_big_skill>` | big-skill；max_output_chars=64 | 0 | output.truncated=true（或有 offload 引用） | 若 offload：落盘文件存在且路径在约定目录 |
| E2E-SK-SEI-05 | run | `deepagents run ... --mock-script <call_panic_skill>` | panic-skill | 0 或 非 0（需固化） | tool_result.error.code=skill_panic | 进程不崩溃；stdout 仍为 JSON |
| E2E-SK-SDX-01 | init+validate | `deepagents skill init <dir> && deepagents skill validate --source <parent>` | init 生成 | 0 | validate 输出包含 skill 元数据摘要 | init 目录结构存在 |
| E2E-SK-SDX-02 | validate | `deepagents skill validate --source <parent>` | 人为破坏 | 非 0 | 输出含 skill 名、文件名、字段路径 | 无 |
| E2E-SK-SDX-03 | validate | `deepagents skill validate --source <src1> --source <src2>` | 正常 sources | 0 | stdout JSON 可解析，skills/tools 摘要齐全 | 无 |

需要在实现中明确并固化的“退出码策略”（建议统一）：

- list：成功 0；仅当输入 source 不合法且 strict 时失败非 0
- validate：任何诊断级别的“非法技能包”都应失败非 0（用于 CI）
- run：若运行中存在 tool_result.error 但 runner 仍能输出 final_text，允许 exit 0；若认为“任何技能错误都应失败”也可，但必须统一并在 E2E 中固化

---

## 附录 C：脚本驱动模型的最小脚本样例（示意）

示例仅用于解释 E2E 如何“确定性触发”技能调用；实际脚本格式以系统对外契约为准。

### C.1 inspect_then_call（先断言注入，再调用）

语义：

- 第 1 步：要求 provider 把本轮 request 的 tools/system 快照写入 trace（或通过 mock 的观察能力导出）
- 第 2 步：发起 tool_call 调用 `math-add`
- 第 3 步：final_text 引用 last_tool_result

### C.2 call_missing_required（触发 schema 失败）

语义：

- 第 1 步：发起 tool_call 调用 `echo-skill`，但缺必填字段
- 第 2 步：final_text 固定为 `done`（用于断言“错误不崩溃且可继续”）

