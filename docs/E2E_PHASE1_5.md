# Phase 1.5 E2E 测试计划（Runtime/Provider/Plugin 选型 + 最小闭环 POC）

适用范围：本计划面向 [ITERATION_PLAN.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/ITERATION_PLAN.md#L57-L87) 的 Phase 1.5。目标是验证 Rust 版在 Phase 1.5 能形成“消息 →（provider 推理）→ tool call/skill call → tool 执行 → 结果回填（messages/state）→ 最终响应”的最小端到端闭环，并且 Runtime/Provider/SkillPlugin 三大抽象可替换、行为可回归。效果参考 Python 版本“闭环稳定、provider 可替换、tool 结果可回填并收敛为最终答复”的体验，但不依赖 Python 代码实现细节。

## 0. 当前系统情况（Phase 1.5 已落地的可测对象）

Phase 1.5 的 E2E 计划必须以“当前已经存在且将长期维护”的入口与契约为基线，以免测试变成一次性样例。

- E2E 入口（非交互、可脚本化）
  - CLI：`deepagents run ...`（spawn 子进程、stdout 输出 JSON）：
    - 入口实现：[main.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents-cli/src/main.rs#L117-L173)
    - 输出结构：`RunOutput`：[protocol.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents/src/runtime/protocol.rs#L37-L49)
- Runtime 默认实现：`SimpleRuntime`
  - provider 超时与错误分类、max_steps 终止、tool 执行与回填 messages/state：
    - 实现：[simple.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents/src/runtime/simple.rs#L38-L174)
    - 关键行为点：call_id 生成、arguments 必须是 object、tool error 不终止闭环：[simple.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents/src/runtime/simple.rs#L220-L291)
- Provider：脚本驱动 `MockProvider`（Phase 1.5 的“确定性模型”）
  - 脚本 DSL 与 step 语义：[mock.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents/src/provider/mock.rs#L8-L105)
  - 细节：脚本 step 的索引与 `last_tool_results.len()` 绑定（一次 step 内多个 tool_call 会“跳步”）：[mock.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents/src/provider/mock.rs#L60-L65)
- Skills 插件：声明式 `DeclarativeSkillPlugin`（manifest → tool_calls）
  - manifest 与 merge_args 行为：[declarative.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents/src/skills/declarative.rs#L9-L95)
- 现有 Phase 1.5 CLI 级 E2E 测试（已覆盖的一部分能力）
  - 测试文件：[e2e_phase1_5_runtime.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents-cli/tests/e2e_phase1_5_runtime.rs#L1-L231)
  - 已覆盖：最小闭环、provider 替换（mock2）、非 object arguments 拒绝、provider_timeout 分类、tool 错误可继续 finalize、声明式 skill plugin 触发 tool。

## 1. 范围与完成定义（E2E 角度）

- 必测范围（Phase 1.5 必须证明）
  - Runtime 闭环
    - 单轮与多轮：provider step → tool_calls/skill_call → tool 执行 → 追加 tool message → 再次 step → final
    - 终止条件：final_text / provider_error / provider_timeout / provider_step_error / skill_error / max_steps_exceeded
  - Provider 抽象可替换
    - 同语义、不同 tool_call 形态的 provider（至少 mock 与 mock2）
    - provider_timeout_ms 行为可回归（不会挂死、不会误执行工具）
  - SkillPlugin 机制（当前选型：声明式 skills）
    - 加载成功、触发 tool_calls、skill_not_found 行为
    - call.input 与 manifest arguments 的 merge 规则可回归（overlay 覆盖 base）
  - 回填链路
    - tool_results 的记录（含 call_id 关联）
    - tool message 写回 messages（用于 provider 基于上轮 tool 结果继续推理）
    - state 的演进（至少 FilesystemState 在 write/edit/delete 后可观测）

- 不范围（Phase 1.5 不要求）
  - 真实模型质量、prompt 优化与自动规划质量
  - 多 agent/subagents（Phase 4+）
  - 长期记忆与 summarization（Phase 7/8）

- 完成定义（门禁）
  - `cargo test -p deepagents-cli` 在本地/CI 重复执行稳定，全用例隔离 root，无外部网络依赖
  - E2E 套件覆盖下文“必测用例组”，并能解释每一条与 Phase 1.5 验收项的对应关系
  - 输出 JSON 契约稳定（字段存在性、错误码分类、call_id 关联规则不随实现漂移）

## 2. 测试入口与输出契约（以当前实现为准）

### 2.1 CLI 命令与注入点

- `deepagents [--root <path>] [--shell-allow <pattern>...] run`
  - `--root <path>`：隔离 workspace root（全局参数；位于 subcommand 前）
  - `--shell-allow <pattern>`：允许执行的 shell 命令模式（全局参数；可重复）
  - `--provider mock|mock2`：Phase 1.5 的可回归 provider
  - `--mock-script <path>`：MockProvider 脚本
  - `--plugin <path>`：声明式 skills manifest（可重复传多次）
  - `--max-steps <n>`：终止上限
  - `--provider-timeout-ms <ms>`：provider.step 超时

### 2.2 stdout JSON 契约（RunOutput）

stdout 必须是单个 JSON 对象（不混入日志）。字段契约：

- `final_text: string`：终止为 final_text 时为非空；其他失败路径可为空
- `tool_calls: ToolCallRecord[]`：执行顺序记录；每条必须包含 `call_id`
- `tool_results: ToolResultRecord[]`：与 tool_calls 一一对应；失败时 `error` 字段为 string，`output` 为 null
- `state: AgentState`：最终 state（至少 filesystem 维度在 Phase 1 已固化）
- `error: RuntimeError|null`：失败分类（见 4.5/4.6/4.7）
- `trace: object|null`：至少含 `terminated_at_step` 与 `reason`

退出码契约（CLI 层）：

- 若 `error == null`：进程退出码为 0
- 若 `error != null`：进程退出码为非 0（当前实现：返回 `runtime_error`）：[main.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents-cli/src/main.rs#L164-L172)

## 3. Harness 与 Fixture（如何写出稳定 E2E）

### 3.1 Harness（spawn 二进制，黑盒断言）

测试建议统一采用以下模式（当前已在用）：[e2e_phase1_5_runtime.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents-cli/tests/e2e_phase1_5_runtime.rs#L1-L24)

- `Command::new(env!("CARGO_BIN_EXE_deepagents"))`
- 必须从 stdout 解析 JSON（禁止依赖 stderr/日志文本）
- 断言点优先使用字段语义，不依赖字符串全量匹配（除非是错误码/固定 reason）

### 3.2 统一 Fixture（每个用例一个临时 root）

每个用例创建隔离 root（`tempfile::tempdir()`），推荐预置：

- `README.md`：至少 2 行，首行可用于 `final_from_last_tool_first_line`
- `src/lib.rs`：用于 grep/glob 扩展用例
- `large.txt`：> 200 行，覆盖 read_file offset/limit 与截断（如纳入）

### 3.3 可回归断言策略

- 只断言“必须稳定”的部分：
  - error.code、trace.reason、tool_calls/tool_results 数量与顺序、call_id 关联、state 中某个关键字段存在与值
- 对可能演进的字段，只断言存在性或类型（例如未来 trace 结构扩展）

## 4. MockProvider 脚本 DSL（测试数据契约）

MockProvider 脚本是 Phase 1.5 E2E 的“确定性推理引擎”，必须把 DSL 的语义契约化，以免测试变成难以维护的隐式逻辑。

### 4.1 Script 格式

脚本文件为 JSON，对应 `MockScript { steps: MockStep[] }`：[mock.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents/src/provider/mock.rs#L25-L29)

### 4.2 Step 索引规则（关键）

当前实现用 `idx = min(last_tool_results.len(), steps.len())` 选择 step：[mock.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents/src/provider/mock.rs#L60-L65)

含义：

- 若某个 step 产生 N 个 tool_calls，则 runtime 执行后会追加 N 个 tool_results，下一轮 idx 将增加 N（即脚本会“跳过”中间 step）
- 因此建议脚本遵循“每次 step 只发 0 或 1 个 tool_call”的写法，以获得最直观的可维护性
- 若需要一次 step 内多个 tool_call，脚本文档中必须明确对应的 idx 跳变（避免误解）

### 4.3 Step 类型与语义

- `{"type":"tool_calls","calls":[{"tool_name": "...", "arguments": {...}, "call_id": "c1"?}] }`
  - 生成 provider 的 tool_calls
  - `mock2` provider 会省略 call_id，让 runtime 生成 `call-<n>`：[mock.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents/src/provider/mock.rs#L91-L98)，[simple.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents/src/runtime/simple.rs#L221-L229)
- `{"type":"skill_call","name":"<skill>","input":{...},"call_id":"c1"?}`
  - 生成 provider 的 skill_call；由 runtime 展开为 tool_calls 并执行
- `{"type":"final_text","text":"..."}`
  - 直接终止为 final_text
- `{"type":"final_from_last_tool_first_line","prefix":"..."}`
  - 从上一个 tool_result.output 抽取第一行并拼接（用于验证“tool 结果影响 final”）：[mock.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents/src/provider/mock.rs#L82-L90)
- `{"type":"error","code":"...","message":"..."}`
  - provider_step_error（RuntimeError.code 透传此 code）：[simple.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents/src/runtime/simple.rs#L108-L120)
- `{"type":"delay_ms","ms":200}`
  - 延迟后返回空 final_text（主要用于触发 provider_timeout 分类）：[mock.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents/src/provider/mock.rs#L75-L80)

## 5. 声明式 Skills manifest（插件数据契约）

当前 Phase 1.5 默认技能机制为“声明式 skills”，manifest 格式如下：

```json
{
  "skills": [
    {
      "name": "read_readme",
      "description": "Read README",
      "tool_calls": [
        { "tool_name": "read_file", "arguments": { "file_path": "README.md", "limit": 20 } }
      ]
    }
  ]
}
```

语义：

- `list_skills()` 暴露 name/description（供 provider 决策）
- `call(SkillCall)` 展开为 tool_calls，并将 `SkillCall.input` 作为 overlay 覆盖 manifest 的 `arguments`：[declarative.rs](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents/src/skills/declarative.rs#L58-L79)
- 若 skill 不存在：返回 `skill_not_found`

注意：当前 manifest 解析没有启用 strict deny unknown fields；因此“未知字段是否应拒绝”是一个需要在 Phase 2/6 决策并补齐测试的契约点（见 7.2）。

## 6. E2E 用例清单（按能力域分组，含现状覆盖与增量缺口）

本清单强调“可回归断言点”，并明确哪些已被现有 E2E 覆盖，哪些是 Phase 1.5 结束前应补齐的增量用例。

### 6.1 最小闭环（必须）

- E2E-RT-001：单轮 tool 闭环（read_file）
  - 现状：已覆盖：[phase1_5_minimal_loop_read_file](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents-cli/tests/e2e_phase1_5_runtime.rs#L26-L58)
  - 断言要点：exit=0、tool_calls=1、tool_results=1、final_text 基于 README 第一行
- E2E-RT-002：多轮 tool 调用（至少 2 次）
  - 增量：补齐（建议脚本一次 step 只发 1 个 tool_call，避免 idx 跳变难读）
  - 断言要点：tool_calls 按顺序为 read_file(README) → read_file(large offset/limit)；final_text 同时引用两个工具结果
- E2E-RT-003：无 tool 直接回答
  - 增量：补齐（mock script 直接 final_text）
  - 断言要点：tool_calls 为空、tool_results 为空、state 为默认/不变

### 6.2 Provider 抽象可替换（必须）

- E2E-PROV-001：mock provider 可插拔
  - 现状：由 E2E-RT-001 覆盖（provider=mock）
- E2E-PROV-002：mock2 provider 省略 call_id，runtime 生成
  - 现状：已覆盖：[phase1_5_provider_replacement_mock2](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents-cli/tests/e2e_phase1_5_runtime.rs#L60-L90)
  - 断言要点：tool_calls[0].call_id == "call-1"
- E2E-PROV-003：provider_timeout 分类
  - 现状：已覆盖：[phase1_5_provider_timeout_is_classified](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents-cli/tests/e2e_phase1_5_runtime.rs#L126-L155)
  - 断言要点：exit!=0、error.code=="provider_timeout"、trace.reason=="provider_timeout"
- E2E-PROV-004：provider_error 分类（provider.step 返回 Err）
  - 增量：补齐（需要 MockProvider 增加一种“直接返回 anyhow Err”的 step，或新增 provider stub）
  - 断言要点：error.code=="provider_error"，trace.reason=="provider_error"
- E2E-PROV-005：provider_step_error 分类（ProviderStep::Error）
  - 增量：补齐（脚本 step type=error 即可）
  - 断言要点：error.code 透传 script.code，trace.reason=="provider_step_error"

### 6.3 Tool call 解析鲁棒性（必须）

- E2E-TCALL-001：arguments 非 object 时拒绝执行但不中断闭环
  - 现状：已覆盖：[phase1_5_tool_call_parsing_rejects_non_object_arguments](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents-cli/tests/e2e_phase1_5_runtime.rs#L92-L124)
  - 断言要点：tool_results[0].error 包含 "invalid_tool_call"
- E2E-TCALL-002：unknown tool 名称（call_tool_stateful 返回错误）
  - 增量：补齐
  - 断言要点：tool_results[0].error 包含 "unknown tool"；后续仍可 final_text（对齐当前“工具错误不终止闭环”策略）
- E2E-TCALL-003：缺字段/错类型（工具 schema 校验）
  - 增量：补齐（例如 read_file 缺 file_path、limit 为字符串）
  - 断言要点：tool_results[0].error 包含 "missing field"/"invalid type" 或 tool 层约定的 schema 错误文本；不 panic
- E2E-TCALL-004：路径越界请求（../ 或 symlink 逃逸）
  - 增量：补齐
  - 断言要点：tool_results[0].error 包含 permission_denied/invalid_path；final_text 稳定

### 6.4 Tool 执行错误与回填（必须）

- E2E-TOOL-ERR-001：file_not_found
  - 现状：已覆盖：[phase1_5_tool_error_is_recorded_and_run_can_still_finalize](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents-cli/tests/e2e_phase1_5_runtime.rs#L157-L188)
- E2E-TOOL-ERR-002：is_directory
  - 增量：补齐
  - 断言要点：error 文本包含 is_directory；final_text 可继续收敛
- E2E-TOOL-ERR-003：execute allow-list 拒绝（为 Phase 2 打基础）
  - 增量：补齐（通过全局参数注入：`deepagents --shell-allow <pattern> run ...`；未配置 allow-list 时走默认行为）
  - 断言要点：command_not_allowed；不会执行到系统命令副作用

### 6.5 Skills/Plugin（当前选型：声明式）

- E2E-PLUG-DECL-001：skill_call 展开为 tool_call 并执行
  - 现状：已覆盖：[phase1_5_declarative_skill_plugin_can_trigger_tool](file:///Users/dafengxu/Dev/deepagents/rust/crates/deepagents-cli/tests/e2e_phase1_5_runtime.rs#L190-L231)
- E2E-PLUG-DECL-002：skill_not_found（provider 发起不存在的 skill）
  - 增量：补齐
  - 断言要点：exit!=0、error.code=="skill_not_found" 或 trace.reason=="skill_error" 且 message 可诊断
- E2E-PLUG-DECL-003：merge_args overlay 覆盖 base
  - 增量：补齐（manifest 里 file_path 指向 README，call.input 覆盖为另一个文件）
  - 断言要点：实际 read_file 读取的是 overlay 指定的 file_path

### 6.6 Runtime 终止条件与 trace（必须）

- E2E-TERM-001：max_steps_exceeded
  - 增量：补齐（mock script 永远 tool_calls 或永远 final_text 为空并不终止；设置 --max-steps=1）
  - 断言要点：exit!=0、error.code=="max_steps_exceeded"、trace.reason=="max_steps_exceeded"
- E2E-TERM-002：final_text 终止时 trace.reason=="final_text"
  - 增量：补齐（script 直接 final_text）

### 6.7 State 回填（Phase 1.5 必须“用得起来”）

Phase 1 已完成 FilesystemMiddleware state 回填；Phase 1.5 的闭环要证明 runtime 执行 tool 时 state 会演进，且最终输出可观测。

- E2E-STATEFUL-001：write_file → state.filesystem 出现文件快照
  - 增量：补齐（tool_call: write_file，final_text 任意）
  - 断言要点：out.state.filesystem.files 包含新文件，且 content 与写入一致或符合截断契约
- E2E-STATEFUL-002：edit_file → state 更新（occurrences=1）
  - 增量：补齐
- E2E-STATEFUL-003：delete_file → state 标记删除或移除（取决于 Phase 1 契约）
  - 增量：补齐

## 7. 迭代拆分（Phase 1.5 期间如何逐步把 E2E 补齐）

### 7.1 建议的迭代门禁（按“先闭环、再鲁棒、再覆盖边界”）

- I1：闭环门禁（已达成）
  - E2E-RT-001、E2E-PROV-002、E2E-PROV-003、E2E-TCALL-001、E2E-TOOL-ERR-001、E2E-PLUG-DECL-001
- I2：鲁棒性门禁（Phase 1.5 必须补齐）
  - E2E-RT-002、E2E-RT-003
  - E2E-TCALL-002/003/004
  - E2E-TOOL-ERR-002
  - E2E-TERM-001/002
- I3：state 可用性门禁（Phase 1.5 结束前建议补齐）
  - E2E-STATEFUL-001/002/003

### 7.2 需要在文档中明确、否则测试无法稳定的“契约点”

- 技能 manifest 是否 strict（未知字段/缺字段是拒绝还是忽略）
- tool schema 校验失败的错误文本/错误码形态（目前以 serde 错误字符串为主）
- “工具错误是否终止 runtime”的策略（当前：不终止，记录 tool_results.error 并继续）
- provider_error 的可测试注入方式（是否需要新增 provider stub 或扩展 MockProvider）
