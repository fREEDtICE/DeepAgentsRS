# Phase 5 详细迭代计划（PatchToolCallsMiddleware：兼容层）

适用范围：本计划面向 [ITERATION_PLAN.md](ITERATION_PLAN.md#L188-L204) 的 Phase 5。目标是补齐 Rust 版 deepagents 的“tool call 兼容层”，把来自不同 provider/runtime/协议形态的 tool call 与 tool result 统一成可回归、可 round-trip 的标准形态；并在 run 开始阶段修复历史 messages 中的“悬挂 tool_calls（dangling tool_calls）”，避免续跑/恢复时出现重复执行或 UI/日志无法对齐。

本阶段对齐锚点来自总计划中的关键契约点（必须显式对齐）：

- “运行前修复 dangling tool_calls（PatchToolCalls 语义）”：见 [ITERATION_PLAN.md](ITERATION_PLAN.md#L14-L18)
- Core 验收基线（PatchToolCalls）：[patch_tool_calls.md](../acceptance/patch_tool_calls.md)
- Python 参考实现（行为参考，不依赖实现细节）：[patch_tool_calls.py](../../../deepagents/libs/deepagents/deepagents/middleware/patch_tool_calls.py)

本计划同时覆盖“ID 清洗”与“错误字段兼容”的最小落地，使 Phase 6/7/8（skills/memory/summarization）在面对不同上游 tool call 格式时不必各自实现兼容逻辑。

## 0. 完成定义（Definition of Done）

Phase 5 完成必须同时满足：

- PatchToolCallsMiddleware 落地（Rust）：
  - 运行开始阶段（第一轮模型调用之前）扫描历史并修复悬挂 tool_calls（PT-01~PT-05 全通过）
  - 修复方式为注入 ToolMessage（tool_call_id 对齐），且该补齐消息可机器识别为“补丁/取消”，并不会触发真实工具执行
- Tool call/response 归一化能力落地：
  - 兼容多种输入形态（字段名差异、arguments 类型差异、缺失 call_id 等）
  - 输出标准形态一致且可 round-trip（同一输入多次归一化不会继续变化）
- 不引入越权路径：
  - call_id 清洗/路径安全规则可回归（不允许通过 call_id 影响文件路径/越界）
  - 补齐消息不会触发任何 backend 副作用
- 测试：
  - E2E/PT 用例集合落地（PT-01~PT-05）
  - 归一化回归用例集合落地（含 round-trip、ID 清洗、错误字段兼容）

## 1. 范围与非目标（Scope / Non-goals）

范围（Phase 5 必做）：

- PatchToolCallsMiddleware：修复历史悬挂 tool_calls（before first provider.step）
- Tool call/response 归一化：字段统一、arguments 解析、call_id 生成与清洗、错误字段兼容
- 为后续阶段提供稳定的标准形态（统一类型/JSON schema），并提供测试矩阵

非目标（Phase 5 不做，但需留好接口）：

- Large tool result offload（`/large_tool_results/...`）的具体落地（可在 Phase 5 预留 ID 清洗工具函数）
- SummarizationEvent / 历史裁剪（Phase 8）
- Memory/Skills 的 state 隔离规则扩展（Phase 6/7）

## 2. 当前系统基线与缺口（Phase 5 启动时）

### 2.1 已有能力（可复用）

- Runtime 与 tool 执行闭环已稳定：
  - provider 产生 tool calls：`ProviderStep::ToolCalls`（见 [provider/protocol.rs](../../crates/deepagents/src/provider/protocol.rs)）
  - runtime 记录 tool_calls/tool_results 并输出 RunOutput（见 [runtime/protocol.rs](../../crates/deepagents/src/runtime/protocol.rs)）
- runtime-level middleware 扩展点已具备（Phase 4 之后的实现已包含）：
  - `RuntimeMiddleware::before_run`：允许运行前扫描/修补历史 messages（见 [RuntimeMiddleware](../../crates/deepagents/src/runtime/protocol.rs#L99-L115) 与 [SimpleRuntime](../../crates/deepagents/src/runtime/simple.rs#L93-L114)）
  - `RuntimeMiddleware::patch_provider_step`：允许在 provider.step 返回后统一修补 ToolCalls/SkillCall（见 [SimpleRuntime](../../crates/deepagents/src/runtime/simple.rs#L167-L187)）
  - `RuntimeMiddleware::handle_tool_call`：允许拦截并短路某个 tool（Phase 4 的 `task` 即依赖该入口）
- Message/ToolCall 已具备结构化承载能力：
  - `types::Message` 支持 `tool_calls/tool_call_id/status/name` 等可选字段（见 [types.rs](../../crates/deepagents/src/types.rs#L3-L25)）
- “兼容层”关键实现已存在且可复用/演进：
  - `ToolCompat`：`normalize_messages / normalize_tool_call_for_execution / tool_results_from_messages`（见 [tool_compat.rs](../../crates/deepagents/src/runtime/tool_compat.rs)）
  - `PatchToolCallsMiddleware`：`before_run` 修补悬挂 tool_calls，`patch_provider_step` 归一化 ToolCalls（见 [patch_tool_calls.rs](../../crates/deepagents/src/runtime/patch_tool_calls.rs#L118-L146)）
  - CLI `run` 已默认安装 PatchToolCallsMiddleware（见 [main.rs](../../crates/deepagents-cli/src/main.rs#L340-L360)）

### 2.2 关键缺口（Phase 5 必补）

以当前代码为 ground truth，本阶段“从 0 到 1”的主链路已具备，但仍有若干必须在 Phase 5 冻结/补齐的契约与回归强度缺口（避免 Phase 6/7/8 继续各自做兼容）：

- 归一化与修补行为需要进一步“契约化 + 黑盒化”：
  - 将“patched 结果”的最小可观测语义固定（`status=patched` + `error` 前缀/枚举 + 无副作用），并与 [patch_tool_calls.md](../acceptance/patch_tool_calls.md) 与 [E2E_PHASE5_PATCH_TOOL_CALLS.md](../e2e/E2E_PHASE5_PATCH_TOOL_CALLS.md) 对齐
  - 将“missing/empty call_id、duplicate call_id”的处理策略固定并落测试（当前实现倾向生成 `call-{n}`，但缺少黑盒契约与回归覆盖）
- 归一化实现存在重复与分叉风险：
  - `normalize_provider_tool_calls` 与 `normalize_tool_call_for_execution` 逻辑重叠（call_id 生成、arguments string/null 修补），需要收敛为单一来源并保证 round-trip
- call_id 清洗策略需要从“工具函数 + 单测”升级为“明确的使用边界”：
  - 当前已存在 `sanitize_tool_call_id`（见 [patch_tool_calls.rs](../../crates/deepagents/src/runtime/patch_tool_calls.rs#L7-L34)），但尚未形成“何时必须清洗/何时必须保留原始 id”的硬约束（这会直接影响 Phase 8 历史落盘与未来 large tool result offload 的安全口径）
- 可诊断性与可审计性补强（推荐）：
  - 为 run 输出的 `trace` 增加 patch 统计（patched_count、normalized_calls_count、dropped/invalid_count 等），以便 ACP/CLI/测试做稳定断言且不依赖日志

## 3. 对外契约（必须冻结）

### 3.1 标准 ToolCall / ToolResult 形态（Rust 内部标准）

本阶段要冻结一份“内部标准形态”（执行时的最小不变量），供 runtime/middleware/CLI/ACP 共享。注意：当前结构体字段部分为 `Option` 以兼容反序列化，但 Phase 5 要求在“进入执行/进入输出记录”两个关键边界处满足强约束。

- ToolCall（执行前标准形态；不变量）
  - `tool_name: String`（允许上游字段别名，最终必须非空；空值归一化为 `"unknown"`）
  - `arguments: object`（必须是 JSON object；若上游为 string/null，归一化必须修复或拒绝）
  - `call_id: String`（必须非空；若上游缺失/为空则按稳定规则生成，例如 `call-{monotonic}`）
- ToolResult（记录/输出标准形态；不变量）
  - `tool_name: String`
  - `call_id: String`（必须非空，且与 ToolCall 对齐）
  - `output: any|null`
  - `error: string|null`（Phase 5 保持 `Option<String>`；结构化 error 的升级留到后续阶段，但必须保证可分类前缀/枚举）
  - `status: "success"|"error"|"patched"`（用于区分真实执行 vs 合成修补）

补充约定（便于对接“content=string”的上游协议形态）：

- tool 角色 Message 的 `content` 允许使用 JSON envelope（当前实现已支持解析）：`{tool_call_id, tool_name, status, output, error, content}`
- 当 `status="patched"` 时，约定 `error` 使用稳定可分类前缀：`tool_call_cancelled:`（当前实现为 `tool_call_cancelled: missing tool result`），便于策略分支与 E2E 断言

要求：

- 进入执行（`ToolCallContext.call_id`）与进入输出记录（`RunOutput.tool_calls/tool_results`）时，`call_id` 必须非空
- `status="patched"` 的 ToolResult 必须保证“不会触发真实执行”，且不产生任何 backend 副作用

### 3.2 PatchToolCalls 的行为契约（对齐验收）

必须对齐 [patch_tool_calls.md](../acceptance/patch_tool_calls.md)：

- 修复发生在 run 开始阶段（第一轮模型调用之前）
- 修复方式：为每个悬挂 tool_call 注入一条 ToolMessage / ToolResult（call_id 对齐）
- 补齐消息必须：
  - 可机器识别为补丁消息（固定前缀或 `status=patched`）
  - 不会触发真实工具执行
- 幂等：历史一致时不应修改 messages；已补齐的不重复补齐

### 3.3 归一化/兼容规则（输入 → 标准形态）

归一化组件必须覆盖以下差异（至少）：

- 字段名差异：
  - `name`/`tool_name`
  - `input`/`arguments`/`args`
  - `tool_call_id`/`call_id`/`id`
- arguments 类型差异：
  - object：直接使用
  - string：尝试 parse JSON；parse 失败则返回 `invalid_tool_call`
  - null/缺失：归一化为 `{}`（仅当工具 schema 允许空对象；否则视为 invalid）
- call_id 缺失：
  - 生成规则必须稳定：`call-{monotonic}` 或 `call-{hash}`（建议 monotonic，与现有 runtime 行为一致）
- tool_result 错误字段差异：
  - `error` 可能是 string/object/缺失；归一化为 `error: Option<String>`
  - 当 error 为 object（例如 `{code,message}`）时序列化成短文本 `"code: message"` 或保留 `details`（若决定引入）

### 3.4 call_id 清洗（安全边界）

冻结一份 call_id 清洗函数（对齐 Python 的 `sanitize_tool_call_id` 思路）：

- 允许字符集：`[A-Za-z0-9._-]`（建议）
- 替换规则：将 `/` `\\` `..` 等路径相关片段替换为 `_`
- 最大长度：建议 128（超出截断；是否追加 hash 后缀可在 Phase 8 再引入）
- 清洗用于“用作路径/文件名/引用键”时；运行中仍以原始 `call_id` 作为对齐键（追溯友好）

## 4. 架构与实现思路（Trait-first）

### 4.1 组件拆分

- `ToolCompat`（纯函数/无副作用，兼容层核心）：
  - 运行前：把“JSON content 编码的 tool_calls / tool_result”提取回结构化字段（便于 PatchToolCalls 与后续阶段复用）
  - 执行前：对 `ProviderToolCall` 做 arguments/call_id/tool_name 归一化，并将不合法调用稳定回填 `invalid_tool_call:*`
  - round-trip：对同一输入重复归一化应保持稳定（至少不继续扩大/二次变形）
- `PatchToolCallsMiddleware`（运行前修补历史）：
  - 输入：`Vec<Message>`（历史）
  - 输出：`Vec<Message>`（可能插入补丁 ToolMessage）
  - 仅做历史一致性修补，不执行工具
- `PatchToolCallsRuntimeHook`（与 runtime 结合的 hook）：
  - 在 `run()` 开始前调用 PatchToolCallsMiddleware
  - 在 provider 返回 tool calls 后，对 tool calls 做归一化（保证 runtime 后续只看到标准形态）

### 4.2 Runtime hook 的最小扩展方案

本仓库当前 runtime-level middleware 已具备 Phase 5 需要的两个扩展点（均有默认 no-op 实现），因此 Phase 5 的重点是“把 PatchToolCalls 的行为固化为可回归契约”，而不是再引入新的 hook：

- `before_run(messages, state) -> messages`：运行第一轮 provider.step 之前修补历史（PatchToolCalls 的主入口）
- `patch_provider_step(step, next_call_id) -> step`：在 provider.step 返回后对 ToolCalls 做归一化（字段别名、arguments string/null、call_id 生成）

需要冻结的关键点是顺序与边界（建议保持当前行为并锁测试）：

- `SimpleRuntime::run` 在执行 middleware 之前先做一次 `normalize_messages`（见 [simple.rs](../../crates/deepagents/src/runtime/simple.rs#L86-L114)）
- PatchToolCallsMiddleware 的 `before_run` 允许再次 normalize（当前实现如此），但应保证幂等与低成本；如后续收敛为单次 normalize，需同步更新回归用例

### 4.3 “悬挂 tool_calls” 在 Rust 的表达选择

Python 的悬挂 tool_call 是 “AIMessage.tool_calls 有 id，但后续没有 ToolMessage(tool_call_id=id)”。Rust 侧当前已同时支持两种历史表示（Phase 5 需要把它们当作“等价输入”处理，并锁定归一化规则）：

- 结构化字段（首选）：`types::Message.tool_calls` 与 `types::Message.tool_call_id/status/name`（见 [types.rs](../../crates/deepagents/src/types.rs#L3-L25)）
- content JSON envelope（兼容输入）：当上游只保存 `content`（字符串）时，通过 `normalize_messages` 从 JSON 中提取 `tool_calls/tool_call_id/status/name`（见 [tool_compat.rs](../../crates/deepagents/src/runtime/tool_compat.rs#L29-L79)）

Phase 5 的契约要求：

- 两种输入形态在进入 `patch_dangling_tool_calls` 前必须被归一化到结构化字段（以避免“只因序列化形态不同而补丁失败/重复补丁”）
- 对“已补齐”的 patched 工具消息，二次 patch 必须幂等（不得再次追加）

## 5. 详细迭代拆解（里程碑）

### 5.1 里程碑与实现映射（当前代码）

本阶段建议以“契约冻结 + 回归增强”为主线推进；实现的主要落点与对应文件如下（用于拆 PR / 建 issue）：

- 历史归一化（messages）：[tool_compat.rs](../../crates/deepagents/src/runtime/tool_compat.rs#L29-L79)
- 悬挂修补（dangling tool_calls）：[patch_dangling_tool_calls](../../crates/deepagents/src/runtime/patch_tool_calls.rs#L36-L84)
- provider step 归一化：PatchToolCallsMiddleware 的 [patch_provider_step](../../crates/deepagents/src/runtime/patch_tool_calls.rs#L133-L146)
- 执行前归一化与拒绝（invalid_tool_call 回填）：[normalize_tool_call_for_execution](../../crates/deepagents/src/runtime/tool_compat.rs#L147-L184) 与 [SimpleRuntime::execute_calls](../../crates/deepagents/src/runtime/simple.rs#L321-L369)
- call_id 清洗（路径安全预备件）：[sanitize_tool_call_id](../../crates/deepagents/src/runtime/patch_tool_calls.rs#L7-L34)
- 回归用例：
  - 单测：[phase5_patch_tool_calls.rs](../../crates/deepagents/tests/phase5_patch_tool_calls.rs)
  - 集成测：[integration_patch_tool_calls.rs](../../crates/deepagents/tests/integration_patch_tool_calls.rs)

### M0：冻结标准形态与兼容矩阵

- 输出
  - 明确 ToolCall/ToolResult 标准字段与必填规则（3.1）
  - 明确归一化规则矩阵（3.3）与 round-trip 定义
  - 明确 call_id 清洗规则（3.4）
  - 明确“call_id 缺失/空字符串、duplicate call_id”的固定策略（对齐 [E2E_PHASE5_PATCH_TOOL_CALLS.md](../e2e/E2E_PHASE5_PATCH_TOOL_CALLS.md#L135-L148) 的 ROBUST 用例）
- 验收
  - 归一化测试用例可以按矩阵编写，无歧义

### M1：实现 ToolCompat（纯函数 + 单测）

- 任务
  - 提供 `normalize_messages(messages) -> messages`（从 content JSON 提取 tool_calls/tool_call_id/status）
  - 提供 `normalize_tool_call_for_execution(call, next_call_id) -> Valid|Invalid`（arguments/call_id/tool_name 归一化 + 错误回填）
  - 提供 call_id 清洗函数（含长度限制）
  - 收敛归一化逻辑为单一来源（避免 `normalize_provider_tool_calls` 与 `normalize_tool_call_for_execution` 分叉），并显式覆盖“call_id 空字符串视作缺失”的规则
- 验收
  - 单测覆盖：
    - 字段名差异
    - arguments 为 string/null/object 的兼容
    - 缺失 call_id 的生成稳定性
    - round-trip（normalize 2 次不变）

### M2：实现 PatchToolCallsMiddleware（运行前修补）

- 任务
  - 扫描历史 messages：为每个悬挂 tool_call 注入补丁 ToolMessage
  - 补丁 ToolMessage 必须具备机器可识别标记：
    - `status="patched"`（推荐）或 content 前缀 `PATCHED_TOOL_CALL:`
  - 幂等：已存在匹配 tool message 不重复插入
  - 明确“何种 tool message 视为已对齐”的判定规则（以 `tool_call_id` 为主键；对 status/content 的要求只用于诊断，不参与对齐判定）
- 验收
  - PT-01~PT-05 全通过（见第 6 节）
  - 负向断言：补丁不产生任何 backend 副作用

### M3：Runtime 集成（pre-run + post-provider-step）

- 任务
  - 在 `run()` 开始前调用 PatchToolCallsMiddleware
  - 在收到 ProviderStep::ToolCalls 后做 tool call 归一化（字段别名/缺 id/arguments string/null）
  - 在工具执行前对所有 `ProviderToolCall` 做最终归一化与拒绝：
    - 覆盖 provider 直接返回的 ToolCalls 与 skill 展开产生的 calls
    - 对不合法调用稳定回填 `invalid_tool_call:*`（ToolResultRecord.status=error）并继续循环
  - 对被拒绝的 tool call：生成一条 ToolResultRecord（status=error）并继续循环（对齐 Phase 1.5 的“错误可回填后继续收敛”体验）
- 验收
  - 集成测试：混合输入形态（arguments string/缺 id/字段名不同）仍可稳定执行或稳定报错

### M4：兼容回归用例集合（含 round-trip）

- 任务
  - 建立/维护 `crates/deepagents/tests/phase5_patch_tool_calls.rs`：
    - 覆盖 PT-01~PT-05 + 归一化矩阵（含字段别名、arguments string 解析、ID 清洗、tool result 解析）
  - 维护 `crates/deepagents/tests/integration_patch_tool_calls.rs`：
    - 覆盖 runtime 集成路径（PatchToolCallsMiddleware + 实际 tool 执行闭环）
- 验收
  - `cargo test` 全通过
  - 归一化/补丁行为在 CI 稳定可重复

### M5：黑盒可观测性补强（推荐）

这部分不是 [patch_tool_calls.md](../acceptance/patch_tool_calls.md) 的硬要求，但建议在 Phase 5 就锁住，否则后续 ACP/UI 很容易重新引入“只能看日志定位”的不可测行为。

- 任务
  - 在 `RunOutput.trace` 增加 patch/normalize 统计字段（例如 `patched_tool_calls_count`、`normalized_tool_calls_count`、`invalid_tool_calls_count`）
  - 对“call_id 缺失/空字符串、duplicate call_id、ambiguous 匹配”这类输入，在 trace 中提供可回归的诊断摘要（避免仅靠 error 文案）
- 验收
  - E2E 可仅依赖 JSON 输出断言 patch 行为，不依赖日志
  - 统计字段不包含敏感信息（不回显文件内容、不回显 execute 命令明文）

## 6. 测试计划（验收优先级最高）

以 [patch_tool_calls.md](../acceptance/patch_tool_calls.md) 的 PT-01~PT-05 为主线：

- PT-01：单个悬挂 tool_call 被补齐
  - 构造 messages：assistant(tool_calls=[{id:"x"...}])，无 tool message
  - 断言：run 前被补齐 tool message/tool result（call_id="x"），且不会写文件
- PT-02：多个悬挂 tool_call 全部补齐且顺序一致
- PT-03：历史一致时不应修改 messages（幂等）
- PT-04：仅补齐确实悬挂的 tool_call（跨消息匹配）
- PT-05：补齐消息必须可诊断（status=patched 或固定前缀）

归一化矩阵测试（建议最少集合）：

- N-01：arguments 为 string JSON（可 parse）→ object
- N-02：arguments 为 string 非 JSON → invalid_tool_call
- N-03：缺 call_id → 生成 call-1/call-2（稳定）
- N-04：字段名兼容（name/input/id/tool_call_id）
- N-05：round-trip（normalize 两次不变）
- N-06：call_id 为空字符串/全空白 → 视为缺失并按规则生成（或拒绝，但必须固定）
- N-07：duplicate call_id → 按固定策略处理（拒绝或标记 ambiguous，并可诊断）
- N-08：provider 省略 call_id（建议用 `MockProvider::from_script_without_call_ids` 驱动）→ runtime 输出侧仍能保持 tool_call/tool_result 可关联
- N-09：tool message 仅 content JSON envelope（无 tool_call_id/status/name 字段）→ `normalize_messages` 可提取并参与 patch/对齐

## 7. 风险与取舍（提前声明）

- 输入形态分叉风险：结构化字段与 content JSON envelope 并存，如果归一化/patch 的前置顺序不一致，容易出现“同一历史在不同入口下 patch 结果不同”；必须用幂等/round-trip 测试锁住顺序与效果
- 归一化过度风险：如果把过多“猜测修复”塞进 normalizer，会掩盖上游协议问题；建议只做确定性修复（string JSON 解析、字段别名），其余直接报错
- 补丁可识别性风险：仅靠 content 文案不稳定；必须以结构化 `status=patched` 与稳定 error 前缀/枚举为主断言
- call_id 策略风险：若默认“自动生成 call_id”与“严格拒绝缺失 call_id”之间摇摆，会导致上层（ACP/UI/恢复）行为不稳定；需要在 Phase 5 明确默认策略并提供可回归开关（如 strict/permissive）

## 8. 交付物清单（Deliverables）

- 文档
  - Phase 5 详细迭代计划（本文）
- 代码（实现阶段产出，应与本文一致）
  - ToolCompat（字段别名/arguments/ID/错误兼容 + 历史 content JSON 提取）
  - PatchToolCallsMiddleware（运行前修复悬挂 tool_calls）
  - Runtime 集成（pre-run/post-step hook）
  - 可观测性：RunOutput.trace 的 patch/normalize 统计（若采纳 M5）
  - 测试集：PT-01~PT-05 + 归一化矩阵 + round-trip
