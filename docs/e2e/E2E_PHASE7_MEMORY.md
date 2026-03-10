# Phase 7 E2E 测试计划（MemoryMiddleware：记忆抽象与最小实现，黑盒）

适用范围：本计划面向 [ITERATION_PLAN.md](../iteration/ITERATION_PLAN.md#L227-L244) 的 Phase 7。目标是验证 **记忆抽象（MemoryStore）+ 最小本地实现 + MemoryMiddleware** 的端到端闭环在黑盒层面可回归：记忆被加载、注入到模型、遵守安全与隐私边界、具备可诊断失败语义，并支持最小写入/查询/淘汰能力。

本计划为黑盒 E2E：只关注外部可观察行为与结果，不依赖源码结构、内部模块、线程模型或序列化细节。

对齐锚点（行为优先）：

- Phase 7 详细契约与范围定义：[ITERATION_PHASE7_DETAILED.md](../iteration/ITERATION_PHASE7_DETAILED.md)

---

## 0. 背景与测试目标（黑盒视角）

Phase 7 的记忆能力需要向使用者提供三类“确定且可回归”的体验：

- **可用**：记忆总是可被模型读取（system 注入），且注入内容来源可解释、可诊断。
- **可控**：记忆来源遵守 root/host 路径安全规则；超大记忆、损坏记忆、权限问题都有稳定错误语义。
- **可隐私**：真实记忆内容只对模型可见，不应出现在对外输出、state-file、ACP 结果或子代理继承中。

黑盒原则：

- 不依赖真实 LLM，必须使用确定性模型（ScriptedModel/MockProvider）。
- 不依赖外部网络。
- 不依赖 “内存实现细节”，只断言外部可观测契约（stdout JSON、退出码、文件副作用、注入内容快照）。

---

## 1. 术语与外部可测对象

- **memory source**：一个文件路径（默认约定为 `AGENTS.md`）。支持多个 sources（有序）。
- **记忆注入**：MemoryMiddleware 在运行开始前将记忆拼接后注入 system prompt，并带固定 marker/标签。
- **私有记忆**：真实记忆内容属于运行期私有数据，不应通过外部接口回传。
- **MemoryStore**：可插拔的记忆存储接口，提供写入/查询/淘汰的最小能力。

---

## 2. 完成定义（E2E 角度）

Phase 7 E2E 通过必须满足：

- 存在可脚本化入口可：
  - 启用 MemoryMiddleware 并触发注入（run）
  - 写入/查询/淘汰记忆（memory CLI 或等价入口）
- system prompt 注入可回归：
  - 包含稳定 marker 与边界标签（例如 `<agent_memory>`）
  - 幂等（同一 run/resume 不重复注入）
- 安全与失败语义可回归：
  - 越界访问/host 路径默认拒绝
  - symlink 拒绝
  - 文件过大拒绝
  - file_not_found 可忽略（不阻塞）
  - 其它错误可诊断并可配置 strict/soft 行为
- 隐私边界可回归：
  - 真实记忆内容不会出现在 `RunOutput.state` 或 `--state-file`
  - 不通过 ACP 或子代理继承外泄
- MemoryStore 能力可回归：
  - put/get/query/evict（或 compact）行为正确
  - 结构化存储格式稳定、版本可诊断

---

## 3. 测试入口与 Harness（第三者视角）

本计划推荐以 CLI 为主黑盒入口（ACP 作为补充）。任何入口都必须满足“可脚本化 + stdout 结构化 + 退出码稳定”。

### 3.1 CLI 入口（推荐门禁）

要求提供等价能力（命令名可不同，但语义必须映射清晰并文档化）：

- `deepagents run --memory-source <path>... [--memory-allow-host-paths] [--memory-disable] [--memory-max-injected-chars N]`
  - 使用确定性模型脚本驱动运行，stdout 输出单个 JSON（RunOutput 或等价结构）。
- `deepagents memory put/get/query/compact`（或等价入口）
  - 用于验证 MemoryStore 的写入/查询/淘汰。

stdout/stderr 基线：

- stdout 仅输出单个 JSON 对象（不混日志）。
- stderr 可输出日志，但 E2E 不依赖 stderr 文案（仅用于定位错误时的包含性断言）。

### 3.2 确定性模型（必须）

E2E 必须使用脚本驱动模型，确保每次都产生相同的调用序列：

- Step 1：不调用工具，仅产出 final_text，便于检验“system 注入是否生效”。
- Step 2（可选）：根据 system 注入内容进行可预测输出（例如复述记忆中的固定字符串）。

核心要求是：测试能断言 runner 发给模型的 messages 中确实包含记忆注入块。

---

## 4. Phase 7 统一 Fixture（记忆资产库 + 工作区 root）

建议沉淀如下 fixture（路径可调整，但结构建议固定）：

- `fixtures/memory/valid/AGENTS.md`：正常记忆文件（含可断言标记）。
- `fixtures/memory/large/AGENTS.md`：超大记忆文件（触发截断/拒绝）。
- `fixtures/memory/invalid/`：
  - `corrupt.md`（不可读或编码异常）
  - symlink 文件（指向 root 外）
- `fixtures/memory/store/`：memory_store.json 用于 MemoryStore 的黑盒断言。
- `fixtures/memory/mock_scripts/`：确定性模型脚本集合。

### 4.1 workspace root 模板（每用例隔离）

每用例创建独立 root（避免相互污染），预置：

- `.deepagents/AGENTS.md`：默认记忆来源（可用）。
- `AGENTS.md`：项目级记忆来源（可用）。
- `secret.txt`：敏感文本（用于“不可泄露”断言）。
- `outside_secret.txt`（root 外）：用于越界访问拒绝断言。
- 可选：root 内 symlink 指向 root 外 `outside_secret.txt`。

---

## 5. 结果断言规范（黑盒一致性）

### 5.1 Run 输出（注入可观测）

Run 输出需满足至少以下断言能力：

- system 注入 marker 可见（例如 `DEEPAGENTS_MEMORY_INJECTED_V1`）。
- `<agent_memory>` 边界标签可见，内容包含记忆标记（如 `MEMORY_NEEDLE`）。
- `<memory_guidelines>` 块可见（包含“不要存储秘密”等规则）。
- `state.extra.memory_diagnostics` 可观测（含 loaded_sources、truncated、injected_chars 等）。
- `state` 不包含 `memory_contents`（或等价私有内容字段）。

### 5.2 MemoryStore 命令输出（结构化 + 可定位）

最小断言集合：

- `put` 成功：退出码 0，stdout JSON 中能看到 entry id/key。
- `get/query`：stdout 可解析，返回条目包含 key/value 与时间戳（或等价）。
- `compact/evict`：返回驱逐报告（至少包含被驱逐数量）。

---

## 6. E2E 用例清单（按能力域分组）

说明：

- 用例按“注入/安全/隐私/存储”分组，确保覆盖 Phase 7 验收点。
- 每条用例具备：输入（source/root/script）、步骤（命令）、断言（stdout JSON + 退出码 + 文件副作用）。

### 6.1 注入与幂等（核心）

**E2E-MEM-INJ-001：默认 sources 注入成功**

- 输入：root 内 `.deepagents/AGENTS.md` 与 `AGENTS.md` 均包含可识别标记（如 `MEMORY_NEEDLE_A`/`MEMORY_NEEDLE_B`）。
- 步骤：`deepagents run --root <root> --provider mock --mock-script <no_tool>`
- 断言：
  - system 中包含 marker 与 `<agent_memory>` 块
  - `<agent_memory>` 中同时包含两处记忆标记（按 source 顺序拼接）
  - `memory_diagnostics.loaded_sources == 2`

**E2E-MEM-INJ-002：注入幂等（不重复插入）**

- 输入：同上
- 步骤：同一次 run 内多轮 provider.step（脚本驱动），或通过 resume 机制触发再次注入
- 断言：marker 仅出现一次；system 不重复追加

**E2E-MEM-INJ-003：缺失 source 不阻塞（file_not_found 忽略）**

- 输入：sources 指向一个不存在的 `AGENTS.md`
- 步骤：`deepagents run --memory-source <missing> ...`
- 断言：
  - run 仍成功
  - diagnostics 中 `skipped_not_found` 增加

### 6.2 安全与路径边界（必须）

**E2E-MEM-SEC-001：root 外路径默认拒绝**

- 输入：memory-source 指向 root 外 `outside_secret.txt`
- 步骤：`deepagents run --root <root> --memory-source <outside>`
- 断言：失败退出（非 0），错误语义为 `permission_denied: outside root`（或等价）

**E2E-MEM-SEC-002：host 路径需显式开启**

- 输入：memory-source 指向 `~/AGENTS.md` 或 host 绝对路径
- 步骤：
  - 未开启 `--memory-allow-host-paths`：应拒绝
  - 开启后：允许加载
- 断言：错误码与 loaded_sources 行为可回归

**E2E-MEM-SEC-003：symlink 拒绝**

- 输入：root 内 `AGENTS.md` 是 symlink 指向 root 外
- 步骤：`deepagents run --memory-source <symlink>`
- 断言：失败（permission_denied: symlink not allowed 或等价）

### 6.3 预算与截断（必须）

**E2E-MEM-BUD-001：超大记忆文件拒绝或截断**

- 输入：超大 `AGENTS.md`（超过 max_source_bytes 或 max_injected_chars）
- 步骤：`deepagents run --memory-source <large> --memory-max-injected-chars 2000`
- 断言（二选一必须固化）：
  - 方案 A：硬失败（memory_quota_exceeded）
  - 方案 B：注入截断，diagnostics.truncated=true，且注入内容含“truncated”标记

### 6.4 隐私与不泄露（必须）

**E2E-MEM-PRIV-001：RunOutput 不包含 memory_contents**

- 输入：包含可识别记忆内容 `MEMORY_SECRET`
- 步骤：`deepagents run ...`
- 断言：
  - `RunOutput.state` 中不包含 `memory_contents`
  - `state-file`（若启用）不包含记忆内容

**E2E-MEM-PRIV-002：子代理不继承记忆私有内容**

- 输入：配置 subagent，并启用 memory（根目录含 `MEMORY_NEEDLE`）
- 步骤：父代理调用 `task` 触发子代理
- 断言：子代理输出/trace 不包含记忆正文；父侧可确认子代理未拿到 `memory_contents`

### 6.5 MemoryStore（写入/查询/淘汰）

**E2E-MEM-STORE-001：put/get/query 基础闭环**

- 步骤：
  - `deepagents memory put --key k1 --value v1`
  - `deepagents memory get --key k1`
  - `deepagents memory query --prefix k`
- 断言：
  - get 返回 v1
  - query 返回包含 k1

**E2E-MEM-STORE-002：compact/evict 策略生效**

- 输入：通过连续 put 触发容量上限
- 步骤：`deepagents memory compact`
- 断言：返回驱逐数量 > 0，且被驱逐条目不再可查询

**E2E-MEM-STORE-003：损坏/不可解析存储文件**

- 输入：人为破坏 memory_store.json（非法 JSON）
- 步骤：`deepagents memory query ...`
- 断言：失败且错误码为 `memory_corrupt`（或等价）

---

## 7. 迭代门禁建议（Phase 7）

建议分三道门禁，逐步收敛：

- I1（闭环基线）：INJ-001、INJ-002、STORE-001
- I2（安全与隐私）：SEC-001、SEC-003、PRIV-001、PRIV-002
- I3（鲁棒与资源）：INJ-003、BUD-001、STORE-002、STORE-003

---

## 8. 落地建议（从计划到可执行）

- 测试工程建议（CLI 黑盒）：
  - `crates/deepagents-cli/tests/e2e_phase7_memory.rs`：spawn 二进制、解析 stdout JSON、断言语义与副作用。
- fixture 沉淀：
  - 建议在仓库新增 `fixtures/memory/...`，减少用例里手工拼装。
- flake 控制：
  - 禁止真实网络；所有断言优先检查结构化字段而非日志文本。
- 诊断与可审计：
  - 对失败用例保存 JSON 输出作为 CI artifact，便于定位。

---

## 附录 A：建议固化的错误码/诊断类别（用于黑盒断言）

建议在 JSON 输出中提供稳定位置承载错误码（`error.code` 或 `tool_results[i].error.code`），并在 E2E 中严格断言：

- `invalid_source` / `file_not_found`
- `permission_denied`（outside root / symlink not allowed / host paths disabled）
- `memory_quota_exceeded`
- `memory_corrupt`
- `memory_io_error`
- `memory_not_found`

---

## 附录 B：可执行用例矩阵（CLI 黑盒）

| 用例 ID | 入口 | 命令示例（占位符） | 主要输入 | 退出码 | 必须断言（stdout JSON） | 必须断言（副作用） |
|---|---|---|---|---:|---|---|
| E2E-MEM-INJ-001 | run | `deepagents run --root <root> --provider mock --mock-script <no_tool>` | 默认 sources | 0 | system 含 marker + `<agent_memory>`；diagnostics.loaded_sources=2 | 无 |
| E2E-MEM-INJ-002 | run | 同上（多轮或 resume） | 同上 | 0 | marker 仅 1 次 | 无 |
| E2E-MEM-INJ-003 | run | `deepagents run --memory-source <missing>` | missing | 0 | diagnostics.skipped_not_found>0 | 无 |
| E2E-MEM-SEC-001 | run | `deepagents run --memory-source <outside>` | 越界 | 非 0 | error.code=permission_denied | 无 |
| E2E-MEM-SEC-002 | run | `--memory-allow-host-paths` | host path | 0 | loaded_sources=1 | 无 |
| E2E-MEM-SEC-003 | run | `--memory-source <symlink>` | symlink | 非 0 | error.code=permission_denied | 无 |
| E2E-MEM-BUD-001 | run | `--memory-max-injected-chars 2000` | large | 0 或 非 0 | truncated=true 或 error.code=memory_quota_exceeded | 无 |
| E2E-MEM-PRIV-001 | run | `deepagents run ...` | memory contains secret | 0 | state 不含 memory_contents | 无 |
| E2E-MEM-PRIV-002 | task | `deepagents run ...` + task | subagent | 0 | 子代理输出不含记忆正文 | 无 |
| E2E-MEM-STORE-001 | memory | `deepagents memory put/get/query` | k1/v1 | 0 | get/query 返回 v1 | store 文件变更 |
| E2E-MEM-STORE-002 | memory | `deepagents memory compact` | 超限 | 0 | evicted>0 | store 文件变更 |
| E2E-MEM-STORE-003 | memory | 破坏 store | corrupt | 非 0 | error.code=memory_corrupt | 无 |

