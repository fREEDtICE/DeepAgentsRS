# 工具输出形态收敛：ToolResultEnvelope 技术方案与迭代计划

## 结论（对应 ITERATION_PLAN.md L301）

`ITERATION_PLAN.md#L301-L301` 这一条客观存在，并且指出的问题在仓库内确实成立：当前“工具输出”在不同实现与不同传输路径上存在**结构与语义不一致**，导致模型端需要额外解释/猜测。

但原表述“Python 多为字符串化输出，Rust 多为结构化 JSON”过于简化，实际更贴近以下事实：

- DeepAgentsRS（Rust）把工具输出当作一等公民的 `serde_json::Value`（结构化），并同时维护展示用的 `content` 字符串（双轨）。见 [protocol.rs](../../crates/deepagents/src/tools/protocol.rs#L11-L14)、[simple.rs](../../crates/deepagents/src/runtime/simple.rs)。
- ZeroClaw（Rust）工具结果在底层被 `String` 锁死，失败信息还会被拼进字符串（语义丢失），导致“Rust 也可能是字符串化”。见 [traits.rs](../../../zeroclaw/src/tools/traits.rs#L4-L10)、[execution.rs](../../../zeroclaw/src/agent/loop_/execution.rs#L51-L75)。
- Python 侧（deepagents）工具消息 `content` 往往是可直接展示/喂给模型的字符串，但也支持 list/dict、多模态块等结构化形态，且 CLI 侧会做字符串化展示。见 [filesystem.py](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py)、[tool_display.py](../../../deepagents/libs/cli/deepagents_cli/tool_display.py#L283-L306)。

因此，该问题是**客观且需要解决**的，但应从“语言特性差异”更正为“协议/抽象层选择差异”。

本方案给出一个可落地的收敛方向：定义跨实现的 canonical **ToolResultEnvelope**，在 runtime/provider/渲染层形成清晰边界，减少模型端解释成本与下游兼容分支。

---

## 背景：当前输出链路长什么样（现状盘点）

### A. DeepAgentsRS（Rust）现状：结构化 output + 展示 content 的双轨

- 工具协议：`ToolResult { output: serde_json::Value }`，天然结构化。见 [protocol.rs](../../crates/deepagents/src/tools/protocol.rs#L11-L14)。
- 标准工具几乎都返回结构化 JSON（`serde_json::to_value`）。见 [std_tools.rs](../../crates/deepagents/src/tools/std_tools.rs#L56-L358)。
- runtime 写入 tool role 消息时会构造一个 JSON 字符串 envelope，并在其中同时写入：
  - `output`（结构化 JSON）
  - `content`（展示/喂给模型的字符串）
  - `status/error/tool_call_id/tool_name` 等字段（语义更完整）
  见 [simple.rs](../../crates/deepagents/src/runtime/simple.rs#L701-L708)（以及同文件后续 success/error 分支）。
- 兼容层能把多种 tool message 变体归一化为结构化记录（并推断 status）。见 [tool_compat.rs](../../crates/deepagents/src/runtime/tool_compat.rs#L29-L136)。

这一路径已经非常接近“统一输出协议”的目标形态，是本方案的主要参考。

### B. ZeroClaw（Rust）现状：底层 String 锁死 + 两套回灌编码

- 工具结果结构：`ToolResult { success, output: String, error }`，`output` 强制是 String。见 [traits.rs](../../../zeroclaw/src/tools/traits.rs#L4-L10)。
- 执行层会把错误折叠进字符串（例如 `"Error: {reason}"`），导致 status/error 的结构语义丢失。见 [execution.rs](../../../zeroclaw/src/agent/loop_/execution.rs#L51-L75)。
- 同一套系统里存在两条“回灌给模型”的编码：
  - XML prompt 模板：`<tool_result ...>...</tool_result>` 文本。见 [dispatcher.rs](../../../zeroclaw/src/agent/dispatcher.rs#L106-L117)。
  - Native tool role：`role="tool"` 的 JSON 字符串，但字段只含 `tool_call_id/content`，失败依赖字符串前缀判断。见 [dispatcher.rs](../../../zeroclaw/src/agent/dispatcher.rs#L194-L245)。
- WS 网关不得不同时理解两种格式（XML 与 tool role JSON）。见 [ws.rs](../../../zeroclaw/src/gateway/ws.rs#L54-L108)。

这意味着“统一输出格式”若不改 ZeroClaw 的协议/抽象，只在渲染层做字符串处理，会不可避免地引入大量兼容分支与推断逻辑。

### C. Python（deepagents）现状：展示友好但类型更动态

- 工具消息往往用 `ToolMessage(content=...)` 回灌，content 可能是字符串，也可能是结构化块（含多模态）。见 [filesystem.py](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L138-L160)。
- CLI 展示层会把 tool message 的 content 做字符串化/美化渲染。见 [tool_display.py](../../../deepagents/libs/cli/deepagents_cli/tool_display.py#L283-L306)、[messages.py](../../../deepagents/libs/cli/deepagents_cli/widgets/messages.py#L706-L745)。

Python 侧的问题不在“不能结构化”，而在“协议层未冻结、输出内容可能随渲染变化”，因此仍需要一个跨语言的 canonical envelope 来对齐。

---

## 问题拆解（为什么会增加模型端解释成本）

1. **同一类语义在不同路径中字段缺失**  
   - tool_call_id 在某些编码中缺失（XML 路径）  
   - status/error 在某些编码中缺失（Native tool role 路径）
2. **错误语义被字符串化折叠**  
   - 失败时把 `"Error: ..."` 拼进输出文本，让“失败”只能靠启发式解析；下游无法稳定统计错误类别。
3. **结构化 output 与展示 content 混杂/丢失**  
   - 某些实现只保留字符串 output，结构字段不可机器消费；另一些实现保留 JSON，但模型看到的文本未必稳定。
4. **大输出 offload 与恢复语义无法统一**  
   - DeepAgentsRS 已经有 offload 逻辑（替换成引用模板），但其他实现没有统一的“引用协议/元信息”。
5. **观测事件缺少 tool_call 关联信息**  
   - ZeroClaw 的观察事件只含 tool 名、duration、success，不含 tool_call_id 与错误摘要，不利于诊断与一致性验证。见 [traits.rs](../../../zeroclaw/src/observability/traits.rs#L6-L73)。

---

## 目标与非目标

### 目标

- 定义一个跨实现稳定的 canonical 工具结果协议：**ToolResultEnvelope**
- 明确三层职责边界：
  - runtime：产出结构化 `output`、明确 `status/error`、关联 `tool_call_id`
  - renderer：把 `output` 渲染成稳定、可控的 `content`（模型可见）
  - provider：把 canonical envelope 编码成各家 provider 所需消息形态（tool role / tool_result / content blocks）
- 支持“大输出 offload”与“多模态”在协议层可表达，且模型可见文本仍稳定
- 为对齐验证提供可测试的 fixtures（同一输入在不同实现输出一致）

### 非目标

- 不在本阶段全面重写所有工具实现为强类型返回（允许渐进迁移）
- 不强制所有 provider 使用同一种 tool-calling 协议（允许 provider 适配层差异存在）

---

## Canonical 规范：ToolResultEnvelope

### 1) JSON Schema（概念层）

> canonical envelope 既要能机器消费（结构化 output/error/meta），也要能直接喂给模型（稳定 content）。

```json
{
  "v": 1,
  "tool_call_id": "string",
  "tool_name": "string",
  "status": "ok | error",
  "output": { "any": "json value" },
  "error": {
    "kind": "string",
    "message": "string",
    "retryable": true,
    "details": { "any": "json value" }
  },
  "content": "string",
  "meta": {
    "duration_ms": 12,
    "offloaded": false,
    "offload_ref": "string",
    "output_bytes": 1234,
    "truncated": false,
    "preview": "string",
    "media": [
      { "type": "image", "mime": "image/png", "ref": "string" }
    ]
  }
}
```

约束：

- `status` 必须显式存在，不允许仅靠字符串前缀推断。
- `error` 只在 `status=error` 时出现；`output` 在失败时也允许携带部分输出（例如 stdout）。
- `content` 必须始终存在，用于模型可见；其生成规则由 renderer 决定。
- `meta` 用于跨实现对齐与调试，不要求全部字段都存在，但 `v/offloaded/truncated` 推荐提供。

### 2) Rust 内部类型（建议）

建议把 canonical envelope 作为内部共享类型（可在 DeepAgentsRS crate 内先落地，再抽到共享 crate）：

```rust
pub struct ToolResultEnvelope {
    pub v: u32,
    pub tool_call_id: String,
    pub tool_name: String,
    pub status: ToolStatus,
    pub output: serde_json::Value,
    pub error: Option<ToolError>,
    pub content: String,
    pub meta: ToolMeta,
}
```

在 ZeroClaw 侧若短期不改 `ToolResult.output: String`，也应当至少在 dispatcher/runtime 层补齐 envelope：把原来的 `output: String` 放进 `output={"text": "..."}`

---

## 生成与渲染规则（收敛关键）

### 1) runtime：如何构造 output/error/status

统一规则：

- **成功**：`status=ok`，`error=None`，`output` 为工具返回的结构化值；若工具只返回文本，使用 `{"text": "..."}`
- **失败**：
  - `status=error`
  - `error.message` 为对人类/模型可读的短文本（不包含敏感信息）
  - `error.kind` 为稳定枚举（例如 `timeout|invalid_args|permission_denied|not_found|tool_internal|provider`）
  - 允许 `output` 携带 stdout/stderr 等（例如 `{"stdout": "...", "stderr": "...", "exit_code": 1}`)
- 禁止把失败信息仅拼进 content/输出字符串；必须由 `status/error` 表达。

落地点（可按实现迁移）：

- DeepAgentsRS：在写 tool role message 的位置直接写入 envelope（现已有近似形态）。见 [simple.rs](../../crates/deepagents/src/runtime/simple.rs#L701-L708)。
- ZeroClaw：在 [execution.rs](../../../zeroclaw/src/agent/loop_/execution.rs#L51-L75) 不再把 `"Error: ..."` 作为唯一语义来源；改为保留 `status/error`，并把字符串输出放入 `output.text` 或 `output.raw`。

### 2) renderer：如何生成稳定的 content（模型可见文本）

content 的核心目标是“对模型稳定、可预测、可复现”，因此建议固定模板：

```text
[tool:{tool_name} id:{tool_call_id} status:{ok|error}]
{rendered_body}
```

`rendered_body` 规则：

- 先走“按工具名的专用渲染器”（例如 `read_file/grep/glob/ls/execute/edit_file`）
- 渲染器输入为结构化 `output`，输出为稳定字符串
  - 对列表：逐行输出，保证确定性排序（如有必要）
  - 对对象：按固定 key 顺序输出关键字段（避免 JSON key 顺序导致波动）
- 若没有专用渲染器：降级为“稳定 JSON pretty print + truncation”

truncation/offload 与 content 的关系：

- `offloaded=true` 时，content 应只包含：
  - 引用 ref（例如 `/large_tool_results/...`）
  - 可选预览 preview（头尾若干行）
  - size/hash 等元信息（不应泄露敏感内容）
- `truncated=true` 时，content 必须显式告知“被截断”，避免模型误判完整性。

Python/CLI 展示层可以继续美化，但不应改变 canonical content 的语义结构，最好只做“富文本上色/折叠”。

### 3) provider：如何把 envelope 映射到各家消息格式

原则：provider 只做“序列化与字段适配”，不做“语义推断/渲染”。

- 若 provider 支持 tool role 消息：把 `content` 作为 tool message 的可见内容；同时可把完整 envelope JSON 串作为 tool message content（或作为额外字段）用于机器消费。
  - DeepAgentsRS 已在 tool message content 里放了 JSON 串（含 output/content/error）。这一做法可延续，但应固定为 canonical schema（`v=1`）。
- 若 provider 要求 tool_result 或 content blocks：把 `content` 放进 provider 要求的位置；把 `output/error/meta` 用 provider 的“额外字段/metadata”承载（若无通道，则仍以 JSON 串放入 content）。

兼容策略：

- DeepAgentsRS 的 [tool_compat.rs](../../crates/deepagents/src/runtime/tool_compat.rs#L29-L136) 继续保留，用于吞掉历史变体；但新写入应尽量只产生 canonical 一种形态。
- ZeroClaw 的 WS 网关 [ws.rs](../../../zeroclaw/src/gateway/ws.rs#L54-L108) 可逐步降级：优先解析 canonical envelope，再兜底解析旧 XML/旧 JSON。

---

## 迁移方案（分阶段、可回滚）

> 目标是先把“协议冻结 + 写入侧统一”做起来，再逐步消除读侧兼容与历史分支。

### 阶段 I2-0：冻结协议与 fixtures（门禁前置）

- 输出：
  - 本文档（canonical schema + 渲染规则 + 迁移策略）
  - fixtures：为核心工具准备 `input(args) -> envelope(output/content)` 的 golden 用例（JSON 文件），用于跨实现对齐
- 验收：
  - fixtures 可被 Rust/Python 测试读取并对比（至少覆盖 `execute/read_file/grep/edit_file`）

### 阶段 I2-1：DeepAgentsRS 写入侧收敛到 canonical envelope

- 修改点：
  - runtime 写 tool message 时把 envelope 固定为 `v=1` 且字段齐全
  - `content` 生成走 renderer（先内置最小渲染器 + fallback）
  - offload 的引用模板、meta 字段与 content 模板对齐
- 验收：
  - `tool_compat` 仍能读取旧消息，但新产生消息只出现 canonical 一种形态
  - E2E：同一工具在不同 provider mock 下 content 完全一致

### 阶段 I2-2：ZeroClaw 引入 envelope（不强制先改所有工具 trait）

- 最小可行路线（推荐）：
  - 保留工具 trait 产出 String，但在 execution/dispatcher 层构造 envelope：
    - `output={"text": original_output}`（可尝试 `serde_json::from_str` 成功则放入 `output` 并保留 `raw_text`）
    - `status/error` 显式字段，不再靠 `"Error:"` 前缀
  - 统一回灌编码：Native tool role 优先；XML 仅保留兼容或逐步废弃
- 验收：
  - WS 网关优先解析 canonical tool message，不再需要猜测 status
  - 观测/日志能按 `tool_call_id` 关联一次调用

### 阶段 I2-3：Python/CLI 对齐展示（不改变协议，只对齐渲染语义）

- 修改点：
  - CLI 展示层识别 canonical envelope，优先展示 envelope.content，并可在 UI 里展开 output/error/meta
- 验收：
  - Python 与 Rust 在同一 fixture 下渲染出的 content 文本一致（除了 UI 富文本差异）

### 阶段 I2-4：Observability 与对齐验证门禁

- 修改点：
  - ZeroClaw 的 `ObserverEvent::ToolCall*` 增加 `tool_call_id/status/error_kind`（输出正文可做 hash/preview，避免泄露）
  - SSE/WS 输出事件 JSON 里带上这些字段，便于黑盒对齐测试
- 验收：
  - 黑盒 E2E：tool 调用→tool result→下一轮模型调用，能在日志里稳定串起来

---

## 风险与策略

- 破坏性变更风险：ZeroClaw 工具 trait 若改为 `Value` 会牵动大量工具实现。建议先在 dispatcher/runtime 做 envelope 包装，待稳定后再下沉到 trait。
- 敏感信息泄露：canonical envelope 不应默认携带完整 stdout/stderr/文件内容到观测事件；只在 tool message 中携带，观测层只带摘要与 hash。
- 多模态兼容：`meta.media` 应优先用引用（ref）表达，避免把 base64 直接塞进 content 造成上下文污染。

---

## 与 Phase 9 的关系（落到 I2 门禁）

本文档对应 Phase 9 的 I2“工具结果渲染一致性”，并将其拆成 I2-0~I2-4 的可执行迭代门禁。建议在 [ITERATION_PLAN.md](ITERATION_PLAN.md) 中引用本方案作为 I2 的技术设计依据。
