# Phase 8.5 详细迭代计划：默认中间件顺序对齐（Python → Rust）

适用范围：本计划面向 [ITERATION_PLAN.md](ITERATION_PLAN.md#L263-L294) 的 Phase 8.5。目标是把 Rust 侧“产品默认路径”（CLI/ACP 的默认装配）调整为与 Python `create_deep_agent()` 主 agent 默认中间件顺序一致，并把“运行时层 vs 工具层”的分层边界收敛成可被测试与文档约束的稳定契约。

本阶段的对齐基准来自两处：

- 技术设计（顺序与迁移口径）：[TECH_DESIGN.md:L309-L343](../TECH_DESIGN.md#L309-L343)
- 现状对齐表（CLI 默认顺序与差异解释）：[PYTHON_PARITY_MATRIX.md](../acceptance/PYTHON_PARITY_MATRIX.md)

本阶段强调四条原则：

- 顺序是契约：默认装配顺序必须可被断言，否则“看似对齐”的行为会在边缘用例里崩溃（尤其是 summarization / patch tool calls / tool 输出驱逐互相叠加时）
- 分层不打架：工具层负责“单次工具执行的副作用与状态回填”；运行时层负责“跨轮上下文塑形与策略编排”
- 不破坏现有注入能力：`SimpleRuntime` 仍然保持“上层注入 runtime_middlewares”的可组合性；默认顺序对齐发生在 CLI/ACP 装配路径与（可选）统一 builder 上
- Stub 先对齐后补功能：`TodoList` / `PromptCaching` 先以 noop/stub 对齐顺序与接口，再逐步补齐真实能力

---

## 1. 完成定义（Definition of Done）

Phase 8.5 完成必须同时满足：

- 默认中间件顺序对齐：
  - CLI `run` 的默认注入顺序与 TECH_DESIGN 的 Rust 目标顺序一致
  - ACP 的默认装配路径与 CLI 保持一致（避免“CLI 对齐了但 ACP 不对齐”）
- 分层边界明确且可回归：
  - `FilesystemMiddleware`（工具层）职责不变：工具执行后回填 `AgentState.filesystem`
  - 新增 `FilesystemRuntimeMiddleware`（运行时层）最小能力：提供“超大工具输出落盘引用”的占位接口（允许先返回未启用状态，但接口与数据流要定下来）
- Stub/noop 中间件可用：
  - `TodoListMiddleware` 与 `PromptCachingMiddleware` 存在且默认装配到正确位置
  - 它们在默认配置下不改变现有输出语义（不影响 tool call、tool result、state 演进）
- 可观测性：
  - 默认路径可输出（或通过 debug trace）证明 middleware 顺序与 hook 调用顺序一致
- 文档与验收更新：
  - `ITERATION_PLAN.md`、`PYTHON_PARITY_MATRIX.md` 等同步反映新顺序
  - 至少新增一组测试锁死“默认顺序 + hook 顺序 + 不破坏现有行为”

---

## 2. 背景：为什么“默认顺序”必须进入迭代计划

Python deepagents 的默认行为不是单个 middleware 的局部效果，而是“顺序叠加”的整体效果。例如：

- Summarization 只改变“模型可见 messages（effective messages）”，而 PatchToolCalls 会修补历史 tool_calls；两者顺序变化会影响“被修补的对象”与“被摘要裁剪的对象”
- Filesystem 的工具输出驱逐（未来的 `/large_tool_results/...`）既要发生在 provider 下一步调用前，又要保证 tool result 的可恢复性；它的位置必须固定，否则 offload 与 summarization 组合会出现不可预测的上下文形状
- HITL（交互暂停/恢复）是“暂停点插入”，如果默认顺序不稳定，就无法保证 pause/resume 的幂等与“不重复执行”

因此 Phase 8.5 把顺序当作“产品级契约”固化，而不是实现细节。

---

## 3. 对齐目标：顺序、分层、与 Hook 语义

### 3.1 Python 默认顺序（主 agent）

`TodoList` →（可选）`Memory` →（可选）`Skills` → `Filesystem` → `Subagents` → `Summarization` → `AnthropicPromptCaching` → `PatchToolCalls` →（可选）用户 middleware →（可选）`HITL`

参考：Python `create_deep_agent()` 的装配顺序见 TECH_DESIGN 引用与 parity matrix。

### 3.2 Rust 目标默认顺序（主 agent 对齐版）

`TodoList` →（可选）`Memory` →（可选）`Skills` → `Filesystem` → `Subagents` → `Summarization` → `PromptCaching` → `PatchToolCalls` →（可选）用户 middleware →（可选）`HITL`

重要说明：

- Rust 的“Filesystem”已经有工具层 `FilesystemMiddleware`（工具执行后回填 state），但缺少运行时层等价物；本阶段要补齐 `FilesystemRuntimeMiddleware`（哪怕先 stub），避免“顺序对齐”停留在文案层。
- Rust 目前 runtime middlewares 有多个 hook（例如 `before_run`、`before_provider_step`、`patch_provider_step`、`handle_tool_call`）。本阶段需明确：顺序对齐既包括“middleware 列表顺序”，也包括“每个 hook 的调用顺序”。

---

## 4. 当前系统基线（Phase 8.5 启动时）

### 4.1 Rust 的两层中间件体系

- 工具层 middleware：围绕 `DeepAgent::call_tool_stateful` 的 `before_tool/after_tool`，目前默认启用 `FilesystemMiddleware`，负责工具执行后回填 `AgentState.filesystem`。
- 运行时层 middleware：围绕 `SimpleRuntime` 的 provider 循环，当前 hook 包括：
  - `before_run`：run 开始前改写 messages/state
  - `before_provider_step`：每次调用 provider 前改写 messages/state（用于 summarization/effective messages、tool args 截断等）
  - `patch_provider_step`：provider step 产出后补丁（如 call_id 兼容）
  - `handle_tool_call`：接管某些工具（如 `task`、`compact_conversation`）

### 4.2 现状顺序（以 CLI run 为“产品默认路径”口径）

现状由 parity matrix 记录：CLI run 默认注入为 `PatchToolCalls → Memory → Skills → Summarization → Subagents`（其中 Skills 可选）。

这与目标顺序存在三类差异：

- 缺失：TodoList、PromptCaching、FilesystemRuntimeMiddleware、HITL 交互闭环
- 顺序不一致：PatchToolCalls 在 Rust 默认是第一个，但目标应接近末尾（在 provider 输入塑形之后统一修补）
- 分层不清：Filesystem 在 Python 同时承担“工具集/状态回填/大输出驱逐”，Rust 目前仅实现“工具层状态回填”

---

## 5. 核心设计：把“顺序”从实现细节提升为可断言契约

### 5.1 中间件插槽（Middleware Slots）

定义一个稳定的“插槽顺序”，由装配器排序，而不是由调用方手工排列：

1) TodoList
2) Memory（可选）
3) Skills（可选）
4) Filesystem（runtime 层）
5) Subagents
6) Summarization
7) PromptCaching
8) PatchToolCalls
9) UserMiddlewares（可选）
10) HITL（可选）

做法建议：

- 每个 runtime middleware 声明一个 slot（常量/enum）
- 装配器将 middlewares 归一化排序，并检测重复/冲突（例如同一 slot 多个默认实现）
- CLI/ACP 只调用装配器，避免两边顺序漂移

### 5.2 Hook 顺序契约（同一个 slot 的跨 hook 一致性）

顺序对齐不仅指“列表顺序”，还指：

- `before_run`：按 slot 顺序运行
- `before_provider_step`：按 slot 顺序运行（此处决定 effective messages 的最终形状）
- `patch_provider_step`：按 slot 顺序运行（此处决定 provider step 的最终形状）
- `handle_tool_call`：按 slot 顺序尝试接管（先到先得）

关键点：PatchToolCalls 若要保持“靠后”，就必须在 `before_provider_step` 也运行（否则它只在 `before_run` 运行，会被之后的跨轮塑形逻辑绕开）。Phase 8.5 的计划是把 PatchToolCalls 从“单一 before_run”升级为“同时支持 before_provider_step”（行为与 Python 更接近：尽量靠近模型调用前做最终修补）。

---

## 6. FilesystemRuntimeMiddleware（运行时层）的占位设计

目标：把未来的“超大工具输出落盘引用”（`/large_tool_results/...`）预留成稳定接口与数据流，但本阶段允许默认不开启真正 offload（以降低风险）。

### 6.1 触发位置与数据流

工具输出驱逐需要发生在“下一次 provider 调用前”，因为工具输出已经被写入 messages（role=tool）。因此它应实现 `before_provider_step`：

1) runtime 执行工具 → messages 追加 role=tool
2) 下一轮 provider 调用前：FilesystemRuntimeMiddleware 扫描历史 tool 输出（或最近窗口外输出）
3) 若命中阈值：把大输出写入 `/large_tool_results/<id>.json|.md`（未来实现），并将 messages 中对应段替换为“引用 + 头尾预览”
4) PatchToolCalls 在最后修补 tool_call_id 语义，保证替换不破坏可恢复性

### 6.2 占位接口（Phase 8.5）

- 暂时只做“检测 + 产出可观测事件/诊断”，不改写 messages（默认行为不变）
- 对外暴露：
  - 配置项（阈值、是否启用、落盘前缀）
  - 诊断事件（命中数量、候选大小、是否启用）
- 与 Summarization 组合的契约：无论是否启用 offload，Summarization 都必须假设“工具输出可能已被替换为引用”，因此后续实现必须保证引用模板稳定。

---

## 7. TodoListMiddleware 与 PromptCachingMiddleware（stub/noop 版本）

### 7.1 TodoListMiddleware（stub/noop）

本阶段只对齐顺序与接口，不补齐完整 todo 语义（完整能力会涉及 todo state/reducer、write_todos 工具与 subagent state 合并规则，超出 Phase 8.5）。

建议最小行为：

- 不改写 messages，不增加新工具，不改变 state
- 仅提供可观测性：在 trace 或 diagnostics 中标记“todolist middleware 已按 slot 执行”

### 7.2 PromptCachingMiddleware（stub/noop）

Python 的 `AnthropicPromptCachingMiddleware` 影响模型调用成本与缓存命中，但 Rust 侧 provider 生态尚未建立。Phase 8.5 只对齐：

- 中间件位置（Summarization 之后，PatchToolCalls 之前）
- 未来可扩展接口：为真实 provider caching 保留“围绕 provider 调用”的插桩点（可能需要新增 `around_provider_step` 或 provider wrapper）

建议最小行为：

- 默认 noop：不改写 messages，不影响 provider_step
- 在 trace 中标记其执行位置

---

## 8. HITL：保持“策略 + 交互”接口分离并显式提示

Phase 8.5 不要求实现完整 HITL 交互闭环，但必须做到：

- 当审批策略返回“需要交互式批准”而当前模式不支持交互时：
  - 输出必须显式提示“不支持交互审批”（而不是仅给出模糊错误）
  - 错误码/状态字段需可被上层识别，便于未来接入真正的 interrupt/resume

这条的目的不是改变安全策略，而是让产品行为更可诊断、更符合 TECH_DESIGN 的迁移口径。

---

## 9. 任务拆解（建议按迭代门禁推进）

### I1：定义顺序契约与默认装配器

- 定义 runtime middleware 的 slot 枚举/常量与排序规则
- 增加统一装配器（供 CLI/ACP 复用），并把“用户 middleware”作为插槽注入点
- 为每个 hook 明确“按 slot 顺序调用”的规则，并文档化

### I2：调整 CLI/ACP 默认顺序（不破坏可注入性）

- CLI run 默认 middlewares 改为目标顺序
- ACP 默认装配路径同步改动
- 更新 parity matrix：Rust 默认顺序字段与差异说明

### I3：新增 TodoListMiddleware（stub/noop）与 PromptCachingMiddleware（stub/noop）

- 实现 runtime middleware 类型，声明 slot 并接入装配器
- 默认不改变 messages/state/tool behavior
- 增加“可观测标记”用于测试断言（优先放在 trace/diagnostics，而不是污染用户可见文本）

### I4：新增 FilesystemRuntimeMiddleware（占位实现）

- 实现 runtime middleware 类型，声明 slot
- 默认只做检测与诊断事件输出（不改写 messages）
- 预留未来 offload 的配置结构与输出模板位置（但本阶段可返回“未启用”）

### I5：HITL 提示收敛

- 在不实现交互闭环的前提下，让 require/interrupt 场景输出“明确不可交互”的诊断信息
- 保证不影响 deny-by-default 的安全策略

---

## 10. 测试计划

### 10.1 单元测试（强约束顺序契约）

- 默认装配器输出顺序测试：给定启用/禁用的可选项，输出 slot 顺序严格匹配目标
- hook 顺序测试：构造一组“记录执行顺序”的 stub middleware，断言：
  - `before_run`/`before_provider_step`/`patch_provider_step`/`handle_tool_call` 都遵循相同 slot 顺序

### 10.2 集成测试（不破坏现有行为）

- CLI run 用 mock provider 脚本回归：相同脚本在调整顺序前后输出应等价（允许新增 trace/diagnostics 字段）
- 关键链路回归：
  - task 子代理 + summarization 同时启用
  - patch tool calls + summarization 同时启用
  - execute require 场景能得到明确“不支持交互审批”提示

---

## 11. 风险与对策

- 风险：仅调整“middleware 列表顺序”但未定义“hook 顺序契约”，导致不同 hook 里顺序不一致
  - 对策：slot + 装配器 + hook 顺序测试三件套一起落地
- 风险：PatchToolCalls 仍只在 before_run 运行，无法对齐“靠近 provider 调用前修补”的口径
  - 对策：要求 PatchToolCalls 同时实现 before_provider_step（或等价机制）并放在 slot 靠后
- 风险：引入 FilesystemRuntimeMiddleware 但与未来 offload 语义不兼容
  - 对策：本阶段只做占位与输出模板位置，先锁定“接口与数据流”再补实现

---

## 12. 交付物清单（Phase 8.5）

- 详细实现与文档：
  - 默认装配器与 slot 顺序契约文档
  - CLI/ACP 默认顺序更新与 parity matrix 更新
- 运行时中间件新增：
  - `TodoListMiddleware`（stub/noop）
  - `PromptCachingMiddleware`（stub/noop）
  - `FilesystemRuntimeMiddleware`（占位）
- 测试：
  - 默认顺序与 hook 顺序的单测
  - CLI/ACP 的非回归集成测试（mock provider）

