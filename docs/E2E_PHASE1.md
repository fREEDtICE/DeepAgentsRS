# Phase 1 E2E 测试计划（契约固化 + FilesystemMiddleware：状态与 schema）

目标：验证 Rust 版在 Phase 1 引入 FilesystemMiddleware 后，工具调用不仅能执行，还能够稳定地产生、更新、合并、删除文件快照 state，并且工具输入输出 schema/错误码/截断信号等契约在端到端路径中保持一致。效果参考 Python 版本的“工具可用性 + 状态可回填 + 可继续分页/继续编辑”的体验，但不依赖其代码实现细节。

## 1. 范围与完成定义

- 范围（Phase 1 必测）
  - 工具 schema 固化：工具入参/出参字段、字段命名、默认值行为、错误码集合、输出模式（grep）、截断信号
  - FilesystemMiddleware state
    - files 快照写入：write/edit 后 state 中出现对应文件
    - reducer 合并：多次更新可合并、可覆盖、支持删除标记（若纳入）
    - 工具与 state 一致性：write/edit/read_file 的闭环一致
  - 分页与截断
    - read_file offset/limit 正确
    - read/execute 触发截断时具备可检测信号，并可继续拉取
  - 端到端路径
    - 一次会话内多次 tool call，state 持续演进且可观测

- 不范围（Phase 1 不要求）
  - 模型推理/自动 tool call（Phase 1.5）
  - subagents/skills/memory/summarization（Phase 4+）
  - ACP 会话协议（Phase 3）：若 ACP 已可用可做加分项，但不作为必需入口

- 完成定义（E2E 角度）
  - E2E suite 可在本地/CI 重复执行，所有用例使用隔离 root，无副作用
  - 覆盖下文核心用例组：state 写入/合并/删除/一致性、schema 校验、错误码、截断/分页

## 2. 测试入口与 Harness 设计（建议）

Phase 1 的关键是 state/reducer。建议 E2E 通过 CLI 增加或使用“会话化 state”入口（内部实现不限，但行为需满足）：

- `deepagents session new --root <root>` → 返回 `session_id`
- `deepagents session tool --session <id> <tool> --input '<json>'` → 返回 `{ output, delta?, state? }`
- `deepagents session state --session <id>` → 返回完整 state（至少包含 files）
- `deepagents session end --session <id>`

替代方案（等价效果）：在现有 `tool` 子命令基础上引入 `--state-file <path>`：

- 每次 tool 调用：读取 state-file → 执行 → 写回 state-file → 输出 `{ output, delta?, state? }`

### 2.1 输出规范（E2E 可断言）

建议每次 tool 调用输出结构化 JSON（字段名可固定为下列之一）：

- `output`：工具输出（原工具返回值）
- `state`：完整 state（可选，但强烈建议在 E2E 模式开启）
- `delta`：本次调用的 state 增量（可选）
- `error`：结构化错误（可选）

E2E 仅依赖这些字段存在与语义，不依赖内部实现。

## 3. Phase 1 统一 Fixture（状态验证友好）

每个用例创建独立 root，并预置：

- `README.md`：含 `needle` 两次
- `src/lib.rs`：含 `needle` 一次
- `empty.txt`：空文件
- `large.txt`：> 500 行（用于分页/截断）
- `bin.dat`：二进制（若 Phase 1 不支持二进制读取，需验证错误码/提示稳定）

复用状态操作序列：

- S1：write_file 新建 → read_file → edit_file → read_file
- S2：write_file 两个文件 → edit_file 修改其中一个 → 删除其中一个（若支持）
- S3：glob/grep 辅助定位 → read_file 分页读取

## 4. E2E 用例清单（按能力域分组）

### 4.1 Schema 与契约一致性

**E2E-SCHEMA-001：所有工具入参 schema 严格校验（缺字段/错类型）**

- 步骤：对每个工具构造缺少必填字段、字段类型错误、额外字段（如启用 strict）
- 期望：返回可分类的 schema 错误（建议 `invalid_input` 或 `schema_validation_failed`）
- 断言：错误包含字段名；无 panic；退出码符合预期

**E2E-SCHEMA-002：工具默认值契约**

- read_file：不传 offset/limit 时默认 limit 生效
- grep：不传 output_mode/head_limit 的默认行为稳定
- execute：不传 timeout 时默认行为明确（无限/默认超时二选一，但必须稳定）

**E2E-SCHEMA-003：错误码集合一致性**

- 构造 file_not_found/is_directory/parent_not_found/no_match/command_not_allowed/timeout 等场景
- 断言：错误码属于约定集合且含义一致（入口不同不改语义）

### 4.2 FilesystemState 写入（核心）

**E2E-STATE-001：write_file 产生 state 文件快照**

- 步骤：会话内 write_file 写入 `<root>/a.txt`
- 期望：state.files 中出现 a.txt 条目
- 断言：至少包含 path；内容可为全量/摘要，但语义需在文档中固定

**E2E-STATE-002：edit_file 更新 state（内容与 modified_at 变化）**

- 步骤：对 a.txt edit_file（替换一次）
- 期望：state 内容更新，occurrences=1
- 断言：modified_at 变化或 delta 明确更新

**E2E-STATE-003：连续更新合并正确（reducer 正确）**

- 步骤：write a.txt → edit a.txt → 再 edit a.txt
- 期望：最终 state 为两次替换后的结果
- 断言：不丢更新、不回退

**E2E-STATE-004：多文件合并正确**

- 步骤：write a.txt；write b.txt；edit b.txt
- 期望：state 同时包含 a 与 b 的最新版本
- 断言：不互相覆盖，不丢条目

### 4.3 FilesystemState 删除语义（若 Phase 1 纳入 deletion marker）

**E2E-STATE-DEL-001：删除标记能移除 state 条目**

- 步骤：写入 a.txt；执行 delete（delete 工具或返回删除标记）
- 期望：state.files 不再包含 a.txt
- 断言：删除可重复（幂等）

若 Phase 1 不实现删除：本组用例标记为非目标，但必须返回明确 `not_supported`（或同语义错误码）。

### 4.4 state 与工具行为一致性

**E2E-CONSIST-001：state 可观测且可驱动后续操作**

- 步骤：write/edit 后读取 session state
- 期望：state 对用户可见，能据此选择下一步工具调用
- 断言：state 与 read_file 不出现明显矛盾（允许 read_file 以磁盘为准，但 state 更新应跟随）

**E2E-CONSIST-002：分页读取与 state 的关系明确**

- 步骤：large.txt read_file limit=50；再 offset=50 limit=50
- 期望：拼接覆盖前 100 行
- 断言：若 state 记录内容，需明确是“全量/摘要/不记录内容”，且行为一致可解释

### 4.5 glob/grep 的结构化输出与可复用性

**E2E-GLOB-STATE-001：glob 输出稳定且可直接用于 read_file**

- 步骤：glob 命中路径列表 → 选一个路径 read_file
- 期望：read_file 成功
- 断言：glob 返回的路径可直接作为 file_path 使用（绝对/相对规则一致）

**E2E-GREP-STATE-001：grep content 输出结构化 + line 1-based**

- 步骤：grep needle，output_mode=content
- 期望：返回数组，元素含 path/line/text
- 断言：line 从 1 开始；text 包含 needle；path 可 read_file

**E2E-GREP-MODE-001：三种 output_mode 的一致性**

- files_with_matches 覆盖 content 的 path 集合
- count 与 content 的数量逻辑一致（考虑 head_limit 截断）

### 4.6 截断与预算

**E2E-TRUNC-READ-001：read_file 超限截断有可检测信号**

- 步骤：触发 read_file 截断（limit 很大或超过字符上限）
- 期望：输出包含 truncation 指示（字段或固定提示文本，需契约化）
- 断言：用 offset/limit 能继续拉取后续内容

**E2E-TRUNC-EXEC-001：execute 输出过大截断且标记 truncated=true**

- 步骤：执行产生大量输出的命令
- 期望：ExecResult.truncated=true，output 被截断
- 断言：不会导致 CLI 崩溃或卡死

### 4.7 安全边界与越界访问（回归）

**E2E-SEC-001：路径越界拒绝（../）**

- 步骤：file_path 使用 `../` 指向 root 外
- 期望：permission_denied 或 invalid_path（契约固定其一）
- 断言：不会泄露 root 外内容

**E2E-SEC-002：符号链接绕行拒绝（若纳入）**

- 步骤：root 内创建 symlink 指向 root 外文件 → read_file
- 期望：拒绝

## 5. 覆盖矩阵（Phase 1 必须覆盖的体验点）

- 工具可用性回归（Phase 0 覆盖项继续通过）
- 状态可回填、可观测、可用于继续操作
- reducer 合并/覆盖/删除语义明确且可回归
- schema/错误码稳定可脚本化
- timeout + truncation 信号避免卡死与输出爆炸

## 6. 落地建议（从计划到可执行）

- 文档：本文件为 Phase 1 E2E 计划基线
- 测试工程（二选一）
  - A：`crates/deepagents-cli/tests/e2e_phase1_stateful.rs`（spawn CLI session/--state-file）
  - B：`crates/deepagents/tests/e2e_filesystem_middleware.rs`（直接驱动 middleware，如果提供可用入口）
- schema snapshot：为工具 schema 生成快照 JSON，作为回归基线（字段变更立刻失败）

## 7. Phase 1 实现前必须明确的“可测试契约点”

为保证 E2E 可落地，Phase 1 需明确：

- state 的可观测接口：session state 或 --state-file
- state.files 的最小字段集合（至少 path；内容/摘要/metadata 的取舍需写入契约）
- deletion marker 是否纳入 Phase 1（若否，明确 not_supported 错误码）
- truncation 信号形态（字段还是固定提示文本）与继续拉取方式
