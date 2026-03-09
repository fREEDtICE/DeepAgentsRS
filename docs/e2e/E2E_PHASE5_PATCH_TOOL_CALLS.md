# Phase 5 E2E 测试计划（PatchToolCallsMiddleware：悬挂 tool_call 修复，黑盒）

适用范围：本计划面向 [ITERATION_PLAN.md](../iteration/ITERATION_PLAN.md#L178-L195) 的 Phase 5：实现 PatchToolCallsMiddleware，用于修复“历史记录中存在 tool_call 但缺少对应 tool_result/ToolMessage”的悬挂情况，确保后续继续运行（resume）时 tool_call/tool_result 对齐稳定。

对齐基准：Python 版本的 PatchToolCallsMiddleware 的可观察行为是“补齐悬挂 tool_call”（为缺失的 call_id 生成一条‘已取消/已补齐’的 ToolMessage），从而保证历史可继续跑、UI 渲染一致、后续轮次不会因为对齐错误而崩溃。

本计划为黑盒 E2E：只关注外部可观察行为与结果，不依赖任何源码结构、内部模块、具体序列化库或中间件实现细节。

---

## 0. 背景与测试目标（黑盒视角）

当系统支持“加载历史继续运行”（例如 CLI run resume、ACP 会话恢复、或从外部存档导入 messages/tool_calls/tool_results）时，常见破坏性输入包括：

- assistant 产生了 tool_calls，但进程在工具执行完成前中断，导致缺少对应 tool_results
- 上游协议/存档只保存了 tool_calls，丢失了 tool_results
- 历史里存在重复/乱序的 call_id，导致 tool_call 与 tool_result 关联歧义

Phase 5 的核心目标是把这类输入修复为“对齐一致”的可运行历史，且修复过程不引入任何副作用（不执行真实工具、不写文件、不触发 execute）。

---

## 1. 术语与约束

- **tool_call**：一次工具调用请求记录，至少包含 `call_id/tool_name/arguments`（字段名以系统对外契约为准）
- **tool_result**：一次工具调用的结果记录，至少包含 `call_id`，并在 `output/error` 二者之一表达结果
- **悬挂 tool_call（hanging tool_call）**：在同一次运行/同一段历史中，存在 tool_call 的 `call_id`，但找不到对应 tool_result
- **patch（修复）**：为悬挂 tool_call 生成“合成 tool_result/ToolMessage”，使其可继续运行且可诊断

约束（必须作为契约）：

- PatchToolCallsMiddleware 只修复对齐，不执行任何工具（包括 read_file/execute 等）
- patch 前后，除新增的合成 tool_result 外，不应改写既有 tool_call/tool_result 的语义内容

---

## 2. 完成定义（E2E 角度）

Phase 5 E2E 通过必须满足：

- 存在可脚本化入口，可注入“包含悬挂 tool_call 的历史输入”，并产出“已修复历史”的可解析输出（JSON）
- 对齐修复可回归：
  - 每个悬挂 tool_call 都会被补齐一个合成 tool_result
  - 合成 tool_result 的 error.code/error.message 稳定可断言
  - 修复过程幂等：对已修复历史再次 patch 不应重复追加
- 安全不退化：
  - patch 不执行工具、不触发任何副作用（尤其是 execute 与文件写入）
  - patch 不应“猜测/纠错”未知工具名来执行（未知工具仍应被标记为取消/缺失，而不是尝试执行）

---

## 3. 必须固化的最小外部契约点（否则 E2E 无法落地）

Phase 5 至少需要对外提供一种“可观测 patch 结果”的方式。允许两种等价入口（二选一或都提供）：

### 3.1 入口选项 A：CLI 提供 patch 命令（推荐）

提供一个非交互命令，输入 JSON（文件或 stdin），输出单个 JSON（stdout，不混日志）：

- `deepagents patch-tool-calls --input <path>`
  - 输入：一段“运行历史/事件序列”（至少包含 tool_calls 与 tool_results，或包含可映射到两者的结构）
  - 输出：修复后的历史（同结构输出，或输出 `{patched_history, stats}`）

优点：完全黑盒、无真实工具执行副作用、定位清晰。

### 3.2 入口选项 B：在 resume/run 链路中启用 patch

允许通过参数/配置启用 PatchToolCallsMiddleware，并在 run 输出 JSON 中体现：

- `tool_calls/tool_results` 对齐已修复（每个 call_id 都能匹配）
- `trace` 或等价字段包含 patch 统计（例如 `patched_tool_results: N`）

优点：同时验证“修复 + 继续运行”链路不会断。

---

## 4. 修复后语义基线（用于 E2E 断言）

为便于端到端断言，本计划要求合成 tool_result 满足最小语义：

- `call_id`：与被修复的 tool_call 的 call_id 完全一致
- `output`：为 null（或缺省），表示未产生任何真实输出
- `error`：必须存在且可分类，建议最小字段：
  - `code: "tool_call_cancelled"`（或等价稳定枚举）
  - `message`：可读文本，至少包含“cancelled/missing tool result”之一（具体文案可变，但需可诊断）

备注：字段名可以调整，但必须能无歧义映射上述语义，并在文档中给出映射关系。

---

## 5. Phase 5 统一 Fixture（历史输入库）

建议以 JSON 文件形式沉淀测试资产：

- `fixtures/patch_tool_calls/cases/*.json`：输入历史（包含悬挂与非悬挂场景）
- `fixtures/patch_tool_calls/expected/*.json`：期望输出（或关键字段断言）

输入历史至少覆盖：

- 单个悬挂 tool_call
- 多个悬挂 tool_call（含跨多轮消息）
- 无悬挂（不应改动）
- 已修复（幂等）
- call_id 重复/缺失（应拒绝或按规则处理，必须契约化）

---

## 6. E2E 用例清单（按能力域分组）

### 6.1 悬挂修复（核心）

**E2E-PATCH-HANG-001：单个悬挂 tool_call 被补齐为合成 tool_result**

- 输入：1 条 tool_call（call_id=c1），tool_results 为空
- 期望：输出中 tool_results 增加 1 条记录，call_id=c1
- 断言：error.code 为稳定枚举（如 `tool_call_cancelled`）；output 为 null

**E2E-PATCH-HANG-002：多条悬挂 tool_call 全部被补齐**

- 输入：多条 tool_call（c1/c2/c3），tool_results 仅包含 c2
- 断言：输出补齐 c1/c3，且不改动既有 c2 结果

**E2E-PATCH-HANG-003：存在对应 tool_result 的 tool_call 不应被再次补齐**

- 输入：tool_calls 与 tool_results 完全对齐
- 断言：输出与输入等价（字节级或语义级，二选一并固化）；patch 统计为 0

**E2E-PATCH-HANG-004：幂等性**

- 步骤：对同一输入连续运行两次 patch
- 断言：第二次输出不应新增更多合成 tool_result；patch 统计为 0

### 6.2 鲁棒性与诊断（必须固化）

**E2E-PATCH-ROBUST-001：tool_call 缺失 call_id 的处理规则固定**

- 输入：某条 tool_call 缺少 call_id（或为空字符串）
- 断言（选择其一并固化）：
  - 方案 A：拒绝并返回结构化错误（如 `invalid_tool_call_id`）
  - 方案 B：生成稳定 call_id 并补齐（需要在输出/trace 中暴露映射，且必须可回归）

**E2E-PATCH-ROBUST-002：重复 call_id 的处理规则固定**

- 输入：两条 tool_call 共享同一 call_id
- 断言（选择其一并固化）：
  - 方案 A：拒绝并返回 `duplicate_call_id`
  - 方案 B：只允许其中一条被关联，其余标记为 `ambiguous_tool_call_id`（必须可诊断）

### 6.3 安全与无副作用（必须）

**E2E-PATCH-SEC-001：patch 不执行任何工具**

- 输入：包含悬挂 execute/read_file 等 tool_call（任意 tool_name）
- 断言：patch 过程不产生文件副作用、不写审计、不执行命令；仅输出合成 tool_result

**E2E-PATCH-SEC-002：unknown tool 也只能被取消/补齐，不能被猜测执行**

- 输入：tool_name 为未知值（例如 `readfile`）
- 断言：不会尝试执行；输出为合成 tool_result（error.code 为 `tool_call_cancelled` 或等价稳定枚举）

---

## 7. 结果断言规范（黑盒一致性）

E2E 断言聚焦：

- tool_calls 与 tool_results 通过 call_id 完全可关联（不存在悬挂）
- 合成 tool_result 的 error.code/error.message 可分类且可诊断
- patch 幂等、无副作用

---

## 8. 落地建议（从计划到可执行）

- 若允许新增入口，优先提供“只做 patch、不执行工具”的 CLI 命令（第 3.1 节），把“历史修复”与“工具执行/运行闭环”解耦，E2E 更稳。
- 如需在 run/resume 链路验证，建议复用 Phase 1.5 的脚本驱动 provider，制造“中断导致悬挂 tool_call”的可控历史输入，再验证 patch 后可继续运行。
