# Phase 6 详细迭代计划（SkillsMiddleware：技能加载与校验）

适用范围：本计划面向 [ITERATION_PLAN.md](ITERATION_PLAN.md#L206-L227) 的 Phase 6。目标是在 Rust 版 deepagents 现有 `SkillPlugin`（manifest→tool_calls）闭环基础上，补齐“技能生态”的产品化能力：**多 source 发现与覆盖、技能包校验、安全边界、对模型可见的注入（system/tools）、以及 DevX（init/validate）**。本阶段完成后，技能不再只是一条 `--plugin <file>` 的“手工注入”，而成为可配置、可诊断、可回归的扩展体系。

对齐锚点（必须显式对齐，避免只对齐文案而偏离行为）：

- 总计划的 Phase 6 定义：[ITERATION_PLAN.md](ITERATION_PLAN.md#L206-L227)
- Extras 验收索引（技能生态生命周期）：[acceptance_extras/skills/index.md](../acceptance_extras/skills/index.md)
  - 发现与覆盖： [discovery_override.md](../acceptance_extras/skills/discovery_override.md)
  - 加载与注入： [loading_injection.md](../acceptance_extras/skills/loading_injection.md)
  - 执行与隔离： [execution_isolation.md](../acceptance_extras/skills/execution_isolation.md)
  - DevX： [devx.md](../acceptance_extras/skills/devx.md)
- Python 参考实现（行为参考，不依赖实现细节）：[skills.py](../../../deepagents/libs/deepagents/deepagents/middleware/skills.py)

本计划默认采用 **Trait-first + deny-by-default + 可观测输出优先** 的策略：先冻结“加载/校验/覆盖/注入/执行边界”的契约，再落实现与测试，保证 Phase 7/8 不需要各自实现技能兼容逻辑。

---

## 0. 完成定义（Definition of Done）

Phase 6 完成必须同时满足：

- SkillsMiddleware 可用（Rust）：
  - 支持从多个 skills source 加载技能（sources 按序、同名 last one wins），并能诊断最终生效技能清单与来源（通过 SD-01~SD-04）
  - 运行期只加载一次并写入 state（`state.extra["skills_metadata"]`），且不会被 subagent 继承（对齐 `_EXCLUDED_STATE_KEYS` 的 `skills_metadata` 语义）
  - 对模型可见注入落地：在每轮 provider 调用前，模型可“看到”技能（system 注入 + tools/skills 描述注入，至少通过 SL-01~SL-03）
- 校验器落地：
  - 非法技能包拒绝、元数据缺失拒绝、权限越界拒绝（Phase 6 验收要求）
  - 严格字段策略与冲突策略固定：未知字段、工具命名冲突、重复 skill name 的处理规则均可回归（SL-05、SD-02/03）
- 安全与隔离不退化：
  - skills 加载与执行不引入越权（不允许通过 symlink/路径穿越读取敏感文件）
  - skill 执行错误不影响 runner（panic/timeout 大输出等通过 SEI-01~SEI-05）
- DevX 有可回归入口：
  - `deepagents skill init` 能生成可加载的最小技能模板（SDX-01）
  - `deepagents skill validate` 可用于 CI（SDX-02~SDX-03）
- 测试门禁：
  - 单测 + 集成测覆盖关键矩阵，`cargo test` 全通过

---

## 1. 范围与非目标（Scope / Non-goals）

### 1.1 范围（Phase 6 必做）

- 技能包结构与目录约定（source/skill 目录、元数据文件、实现文件）
- SkillsLoader：多 source 发现、覆盖规则、诊断输出
- SkillsValidator：元数据校验、安全审计、权限边界校验
- SkillsMiddleware：加载一次→写入 state→注入模型可见信息→执行时遵守权限/隔离
- CLI 入口：sources 配置、list/validate/init
- 示例技能：
  - 一个只读工具（例如 `read_project_readme`：调用 `read_file`）
  - 一个写文件工具（例如 `write_note`：调用 `write_file`，受控路径）

### 1.2 非目标（Phase 6 不做，但需留好接口）

- WASM/脚本运行时插件（仍属于 Phase 1.5 的长期演进路径，Phase 6 聚焦“生态/契约/安全”）
- 大工具结果 offload（`/large_tool_results/...`）的完整实现（可在 Phase 6 只预留接口与错误码映射；真正落地见后续阶段）
- SummarizationEvent / 历史裁剪（Phase 8）
- Memory 的持久化与检索（Phase 7）

---

## 2. 当前系统基线与 Phase 6 缺口

### 2.1 已有能力（可复用）

- SkillPlugin 抽象已固化：
  - `SkillPlugin::list_skills / call`（见 [skills/protocol.rs](../../crates/deepagents/src/skills/protocol.rs#L27-L31)）
- 已有声明式技能（manifest→tool_calls）最小闭环：
  - `DeclarativeSkillPlugin`（见 [declarative.rs](../../crates/deepagents/src/skills/declarative.rs#L9-L95)）
  - runtime 对 `ProviderStep::SkillCall` 的展开执行（见 [SimpleRuntime](../../crates/deepagents/src/runtime/simple.rs#L222-L319)）
  - CLI `run --plugin <path>` 显式加载 declarative manifest（见 [main.rs](../../crates/deepagents-cli/src/main.rs#L333-L338)）
- runtime-level middleware 扩展点已具备（Phase 5 已在用）：
  - `before_run`（一次性加载/注入的最佳入口）
  - `handle_tool_call`（对“技能工具化/宏工具”的拦截入口）

### 2.2 Phase 6 必补缺口（以验收为准）

对照 [acceptance_extras/skills/](../acceptance_extras/skills/index.md)：

- 缺 skills sources 发现与覆盖规则：
  - 现状：仅支持 `--plugin` 指定单文件，无法表达 sources 列表，也无法诊断覆盖（SD-01~SD-04 缺口）
- 缺技能包校验与安全审计：
  - 现状：declarative manifest 解析不 strict（未知字段会被忽略），且无包级安全审计（symlink/大小/路径越界）
- 缺模型可见注入：
  - 现状：skills 仅通过 provider 协议的 `skills` 列表可见（`SkillSpec`），没有 system prompt 注入块；也没有明确“tools/skills 的稳定顺序与去重规则”（SL-01~SL-03 缺口）
- 缺执行期隔离与权限治理：
  - 现状：skill 展开出来的 tool_calls 会沿用 core tools 的全局权限策略，但缺少“技能自身的 allow-list/deny-by-default”与资源上限（SEI-01~SEI-05 缺口）
- 缺 DevX：
  - 现状：无 `skill init/validate`，无法在 CI 快速校验（SDX-01~SDX-03 缺口）

---

## 3. 对外契约（必须冻结）

Phase 6 的关键是把“技能包是什么、如何发现、如何覆盖、如何注入、如何执行、如何失败”冻结成可回归契约。

### 3.1 skills source 与覆盖规则

冻结规则（直接对齐 SD-01~SD-04）：

- sources 为有序列表（CLI/配置提供），按顺序加载
- 同名 skill 冲突：后加载覆盖先加载（last one wins）
- 生效 skill 集合去重：同名 skill 最终仅保留一个版本，不应残留重复工具/重复注入
- 对无效 source 的语义必须二选一并固化：
  - 推荐默认：启动失败（更安全、更可诊断）
  - 允许配置：跳过并记录告警（用于“可选用户目录”场景）

### 3.2 技能包结构（目录约定）

本阶段推荐采用“一个技能=一个目录”，并以 **SKILL.md + manifest（工具定义）** 的双文件约定兼顾：

```
<source_root>/
  <skill_name>/
    SKILL.md          # 必选：YAML frontmatter + markdown instructions（对齐 Python）
    tools.json        # 可选：声明式工具定义（Phase 6 Rust 执行用）
    assets/*          # 可选：示例、静态资料（只读）
```

设计取舍：

- SKILL.md 用于对齐 Python/生态规范（元数据 + 说明，便于 humans 阅读与审计）
- tools.json 用于“可执行定义”（对 Rust runtime 更稳，避免把可执行结构塞进 markdown 解析）
- 若 source 中仅提供 tools.json（无 SKILL.md），默认拒绝（避免缺少元数据与审计入口）
 - 若某个技能目录缺少 tools.json，则该技能视为“prompt-only skill”：只参与 system 注入与清单展示，不注册任何可调用工具（避免把“缺实现”误当成“可执行工具”）

### 3.3 SKILL.md frontmatter 规范（元数据与约束）

对齐 Python 的可观察约束（参考 [skills.py](../../../deepagents/libs/deepagents/deepagents/middleware/skills.py#L21-L89)）并冻结：

- 必填字段：
  - `name`：1-64，小写字母/数字/`-`，不得以 `-` 开头或结尾，不得含 `--`，且必须与父目录名一致
  - `description`：1-1024
- 可选字段：
  - `license`、`compatibility`（<=500）、`metadata`（string map）、`allowed-tools`（工具白名单，实验字段）
- 安全上限：
  - SKILL.md 最大大小：10MB（防 DoS）
- 解析策略：
  - frontmatter 缺失：默认拒绝（或可选跳过，但需固定；推荐拒绝以通过 SD-04/SDX-02）
  - 字段类型不匹配：拒绝并指出字段路径

### 3.4 tools.json 规范（可执行工具定义）

Phase 6 需要把“技能=可调用工具能力”落地，因此 tools.json 必须冻结成可回归 schema。推荐最小形态：

- 文件：`tools.json`
- 顶层：`{ "tools": [ ... ] }`
- tool 定义：
  - `name: string`（工具名，默认可等于 skill 名，也可一个 skill 目录提供多个工具）
  - `description: string`
- `input_schema: object`（JSON Schema 子集；至少支持 required/props/type/string/number/boolean/object，且默认 `additionalProperties=false`）
  - `steps: [ { "tool_name": "...", "arguments": { ... } } ]`
  - `policy`（可选）：工具级权限（allow_filesystem/allow_execute/allow_network），以及最大 steps、超时、最大输出等

与 SKILL.md 的 `allowed-tools` 字段关系（需冻结，避免“双来源”导致不可回归）：

- `allowed-tools` 仅作为“推荐/提示”（注入到 system 技能块，帮助模型决策），不作为强制执行边界
- 强制执行边界以 tools.json 的 `policy` 为准（deny-by-default），并且仍需叠加全局 CLI/ACP 的 execute 审批与 allow-list 策略（Phase 2 契约不可被技能绕过）

必须冻结的行为：

- unknown 字段：默认拒绝（deny unknown fields），避免 silent ignore
- tool 命名冲突：
  - 与 core tools 冲突：默认拒绝（对齐 SL-05 的方案 A）
  - 与其他 skill tool 冲突：按 sources 覆盖规则解决，但最终必须唯一

### 3.5 模型可见注入契约（system/tools）

本阶段至少需要提供两类可观测注入（对齐 SL-01~SL-02）：

- tools/skills 列表注入：
  - model request 中包含“可调用的技能工具清单”（名称+描述+JSON Schema）
  - 顺序与去重规则固定（建议：core tools 在前，skills tools 按最终生效顺序追加）
- system prompt 注入：
  - messages 中必须出现可诊断的技能块（建议固定前缀 `## Skills`）
  - 至少列出 tool 名 + 简述 + 来源（source name）

注入幂等：

- 多次 resume/run 不应重复注入同一块（建议使用固定 marker，如 `DEEPAGENTS_SKILLS_INJECTED_V1`）

### 3.6 执行与隔离契约（权限/资源/失败语义）

必须对齐 SEI-01~SEI-05：

- deny-by-default：
  - 若工具/技能未显式允许 filesystem/execute/network，则禁止其生成对应的 tool_call（返回 `permission_denied:*`）
- timeout：
  - 每个技能工具执行需有可配置超时（默认值固定），超时后返回 `skill_timeout`
- 大输出：
  - 对超大 output 必须固定策略：截断或 offload（Phase 6 可先选截断；如选 offload 需与 Phase 8 路径模型对齐）
- panic/异常：
  - 技能执行 panic 不应导致 runtime 崩溃，必须转为可诊断错误（`skill_panic`）

---

## 4. 架构与实现思路（Trait-first）

### 4.1 模块拆分

- `skills::loader`（发现/覆盖/读文件，不做执行）：
  - 输入：sources + 选项（strict/skip_invalid）
  - 输出：`LoadedSkills { tools: Vec<SkillToolSpec>, metadata: Vec<SkillMetadata>, diagnostics }`
- `skills::validator`（纯校验 + 安全审计）：
  - 校验 frontmatter、文件大小、命名规则、tools.json schema、命名冲突、权限字段、步骤数限制
  - 可复用已有 call_id/path sanitize 的思路（但不要复用 call_id 作为文件路径）
- `runtime::SkillsMiddleware`（加载一次 + 注入 + 执行约束）：
  - `before_run`：加载并写入 `state.extra["skills_metadata"]`，注入 system 技能块
  - `handle_tool_call`（可选，取决于是否“技能工具化”）：
    - 拦截 tool_name 属于 skill tools 的调用
    - 将其展开为 core tool calls 并执行（或复用现有 SkillPlugin→ProviderStep::SkillCall 路径）

### 4.2 兼容/迁移策略（避免 Phase 1.5 已有能力被废）

Phase 6 推荐提供“双路径兼容”，并在文档里明确优先级：

- 兼容路径 A（保留现有）：`SkillPlugin` + `ProviderStep::SkillCall`
  - 好处：现有 E2E 已覆盖，展开后的 tool_calls 记录更自然
  - 风险：模型可见注入更弱（skills 不一定出现在 tools 列表）
- 新路径 B（Phase 6 目标）：skills tools “工具化”
  - 在 tool_specs 中出现 skills tools，并由 SkillsMiddleware 拦截执行（对齐 SL-01）

建议 Phase 6 的最小闭环采用：

- 对模型可见：至少 system prompt 注入 + `skills` 列表注入（必要时扩展 tool_specs）
- 对执行：优先复用现有 SkillCall 展开，确保 tool_calls/tool_results 可观测；技能工具化作为可选增强（可通过 feature flag 启用）

### 4.3 state 与 subagents 隔离

- 统一写入：`state.extra["skills_metadata"] = [...]`（JSON）
- subagent 隔离：保持 `EXCLUDED_STATE_KEYS` 已包含 `skills_metadata`（见 [subagents/protocol.rs](../../crates/deepagents/src/subagents/protocol.rs#L15-L21)），因此 child 不继承技能元数据（对齐 Python）

---

## 5. 详细迭代拆解（里程碑）

### M0：冻结契约与 schema（文档优先）

- 输出
  - 固化 skills source 覆盖规则（SD-01~SD-04）
  - 固化 SKILL.md frontmatter 约束（name/description/size/字段）
  - 固化 tools.json 最小 schema（deny unknown fields）
  - 固化冲突策略（SL-05 选择方案 A：默认禁止覆盖 core tools）
- 验收
  - 测试用例可直接映射到 SD/SL/SEI/SDX 编号，无歧义

### M1：实现 SkillsLoader + 覆盖与诊断（单测）

- 任务
  - 支持 sources 列表扫描（目录→子目录→SKILL.md）
  - 生成 `SkillMetadata`（name/description/path/source）
  - last-one-wins 覆盖并提供 diagnostics（被覆盖项列表、最终生效项列表）
  - 对 invalid source 的策略固定（默认失败，可配置 skip）
- 验收
  - 单测覆盖 SD-01~SD-04（含覆盖、无效 source）

### M2：实现 SkillsValidator（严格校验 + 安全审计）

- 任务
  - frontmatter 解析与字段校验（长度/格式/目录名一致）
  - 文件大小上限与拒绝策略
  - 目录安全：拒绝 symlink、拒绝越界引用、拒绝非预期文件类型（可复用 zeroclaw 的审计思路）
  - tools.json 校验：deny unknown fields、input_schema 子集校验、steps 数限制、工具命名冲突检测
- 验收
  - 单测：非法技能包拒绝、schema 缺失拒绝、权限越界拒绝（Phase 6 验收要求）

### M3：SkillsMiddleware：加载一次 + system 注入 + state 写入（集成测）

- 任务
  - `before_run`：
    - 如果 `skills_metadata` 已存在则跳过（幂等）
    - 否则加载+校验+写入 state，追加 system message（固定 marker 防重复）
  - 为 CLI/ACP 提供“列出已加载技能清单”的结构化输出：
    - 推荐：在 `RunOutput.trace` 或 `state.extra["skills_diagnostics"]` 中输出摘要
- 验收
  - SL-02 通过（system prompt 包含技能块）
  - SD-01/02 的“来源可诊断”可通过 JSON 输出断言

### M4：执行与隔离（权限/超时/大输出/异常）

- 任务
  - 在技能执行路径加入 policy enforcement（deny-by-default）
  - 超时：技能执行整体超时（即使内部包含多步 tool_calls）
  - 大输出：选择截断或 offload（Phase 6 推荐截断并返回稳定提示）
  - panic 捕获：将 panic 转成 `skill_panic`（不崩溃）
- 验收
  - SEI-01~SEI-05 全通过

### M5：DevX：skill init / validate（CLI + 测试）

- 任务
  - `deepagents skill init <dir>`：
    - 生成最小目录（SKILL.md + tools.json + 示例）
  - `deepagents skill validate <dir|source>`：
    - 只做加载+校验，不启动 runtime
    - 输出包含技能名、工具名、参数摘要；错误含文件路径/字段名
- 验收
  - SDX-01~SDX-03 全通过

### M6：示例技能与回归资产沉淀

- 任务
  - 增加两个示例技能目录（只读/写文件受控）
  - 增加 fixtures：sources A/B 覆盖用例、非法技能包用例、权限不足用例
- 验收
  - README/文档给出最小可运行命令行示例

---

## 6. 测试计划（以验收编号为主线）

### 6.1 必测：发现与覆盖（SD-*）

- SD-01：单 source 加载（能列出技能与来源）
- SD-02：多 source 覆盖（last one wins）
- SD-03：覆盖后旧版本不可被调用（用可区分输出断言）
- SD-04：无效 source 的错误语义（失败或跳过，必须固定）

### 6.2 必测：加载与注入（SL-*）

- SL-01：skills 工具出现在 model tools 中（若采用“技能工具化”；否则以 `skills` 列表注入等价断言并文档化）
- SL-02：system prompt 注入包含技能说明块（固定前缀与来源）
- SL-03：技能可被模型调用并回注（复用 mock provider 的 skill_call 脚本）
- SL-04：技能 schema 错误的可诊断失败（validate + 运行时调用两条路径均可断言）
- SL-05：skills 与 core tools 命名冲突处理（默认禁止，启动失败）

### 6.3 必测：执行与隔离（SEI-*）

- SEI-01：无权限时禁止 filesystem 副作用
- SEI-02：无权限时禁止 execute
- SEI-03：超时与取消（runner 继续）
- SEI-04：大输出处理（截断或 offload；策略固定）
- SEI-05：panic/异常传播语义（不崩溃）

### 6.4 必测：DevX（SDX-*）

- SDX-01：init 生成的技能模板可被加载
- SDX-02：validate 能发现 schema/实现错误（exit code 非 0）
- SDX-03：validate 成功时可用于 CI（exit code 0，输出摘要）

---

## 7. 风险与取舍（提前声明）

- “skills=prompt” vs “skills=tool” 概念漂移风险：Python skills 偏指令注入；Rust 当前 skills 偏“宏展开”。Phase 6 必须在文档中明确：本阶段的 skills 既包含元数据/说明，也包含可执行工具定义（tools.json），并通过注入契约把两者合并为一致体验。
- 注入点不足风险：当前 runtime middleware 只有 `before_run/patch_provider_step/handle_tool_call`，若要“每轮 request 注入 tools/schema”，可能需要新增 hook（例如 `patch_provider_request`）。本阶段优先采用“before_run 注入系统消息 + skills 列表注入”的最小可回归方案，工具化注入作为增强。
- 安全边界复杂度风险：source 目录可能在 sandbox root 外；必须做 canonicalize 与 symlink 拒绝，并在文档中说明“允许的来源范围”与默认策略（失败 vs 跳过）。
- 大输出策略与 Phase 8 耦合风险：如果选择 offload，需要先冻结路径模型与落盘策略；Phase 6 默认选择截断并返回稳定提示，避免提前耦合。

---

## 8. 交付物清单（Deliverables）

- 文档
  - Phase 6 详细迭代计划（本文）
  - tools.json / SKILL.md 的 schema 说明（可附在本文或单独文档）
- 代码（实现阶段产出，应与本文一致）
  - SkillsLoader + SkillsValidator + SkillsMiddleware
  - CLI：skills sources 配置、skill init、skill validate、skill list（或等价入口）
  - 示例技能目录（只读 + 写文件受控）
  - 测试：SD/SL/SEI/SDX 全量用例集（单测 + 集成测/E2E）
