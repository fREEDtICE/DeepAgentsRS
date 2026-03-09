# Phase 0 详细迭代计划（最小工程骨架 + 本地工具闭环）

适用范围：本计划面向 [ITERATION_PLAN.md](ITERATION_PLAN.md#L10-L33) 的 Phase 0，目标是把“最小工程骨架 + 本地工具闭环”拆解为可执行、可验收、可回归的详细迭代计划，并覆盖该阶段应固化的核心设计细节。

## 0. Phase 0 的完成定义（Definition of Done）

Phase 0 完成需同时满足：

- 代码结构：Rust workspace 初始化完成，核心 crate 边界清晰，可独立构建/测试
- 工具闭环：通过 CLI 可调用一组标准工具（ls/read_file/write_file/edit_file/glob/grep/execute）
- 行为契约：关键行为可稳定断言（read_file 行号输出、grep 字面量、execute exit_code+output 等）
- 安全边界：本地 sandbox 的 root 边界与越界拒绝已具备（至少对路径越界有效）
- 测试基线：单元测试通过；具备 Phase 0 E2E 测试计划文档（并预留落地入口）
- 文档可复现：在干净环境按文档可复现构建、运行、基本验证流程

## 0.1 与当前实现对照（代码为准）

Phase 0 的核心交付在当前代码中已落地，主要位置如下（后续阶段文档请以这些实现为 ground truth）：

- 默认工具集与 schema/输入校验：`ls/read_file/write_file/edit_file/delete_file/glob/grep/execute`，见 [std_tools.rs](../../crates/deepagents/src/tools/std_tools.rs)
- 本地 sandbox 后端（root 边界、文件操作、execute allow-list/危险模式）：见 [local.rs](../../crates/deepagents/src/backends/local.rs)
- CLI `tool` 子命令（JSON in/out + `--state-file`）：见 [main.rs](../../crates/deepagents-cli/src/main.rs#L30-L160)

## 1. 交付物清单（Deliverables）

- Workspace
  - `/rust/Cargo.toml`：workspace members 与统一依赖版本
  - `/rust/README.md`：开发入口说明
- Core SDK
  - `deepagents` crate：backend/tool/agent 的最小闭环
  - 本地 sandbox：`LocalSandbox`
  - 默认工具集：与 CLI 对接的标准工具集合
- CLI
  - `deepagents-cli`：提供 `tool` 子命令，JSON 输入输出稳定
- 文档
  - Phase 0 技术设计基线：[TECH_DESIGN.md](../TECH_DESIGN.md)
  - Phase 0 E2E 测试计划：[E2E_PHASE0.md](../e2e/E2E_PHASE0.md)
  - 本文：Phase 0 详细迭代计划

## 2. 设计细节（Phase 0 必须固化的最小决策）

### 2.0 Trait 优先原则（Phase 0 的落地方式）

Phase 0 的核心价值是把“可扩展点”固化为 trait 边界，而不是把某个具体实现做大做全。该阶段必须遵守：

- 先定义 trait 与契约：`FilesystemBackend` / `SandboxBackend` / `Tool`
- 再提供默认实现：`LocalSandbox` 与默认工具集仅作为参考实现与开发默认值
- CLI 只做胶水：CLI 不包含业务逻辑，只依赖 trait 暴露出来的能力

### 2.1 Workspace 与 crate 边界

- workspace members（建议至少三类）
  - `deepagents`：核心库，禁止依赖 CLI/TUI
  - `deepagents-cli`：命令行入口，仅依赖 `deepagents`，不引入业务逻辑
  - `deepagents-acp`：Phase 0 只保留骨架，不进入协议细节

- 依赖管理原则
  - 统一版本放到 workspace 级别，避免依赖分裂
  - Phase 0 依赖尽量少：serde/serde_json、tokio、walkdir、globset、regex（安全校验用）、tracing

### 2.2 Backend 抽象（最小可扩展点）

- Phase 0 目标不是做“完整 backend 体系”，而是定出后续可扩展的 trait 边界：
  - `FilesystemBackend`：文件与搜索
  - `SandboxBackend`：在其上扩展 `execute`

- API 语义约束（Phase 0 必须遵守）
  - root 边界：任何输入路径最终解析都不得越出 root
  - read 分页：offset/limit 以行计；输出行号为 1-based，格式稳定可断言
  - grep 为字面量匹配：pattern 不视为正则
  - glob 返回绝对路径列表：排序稳定
  - write_file 仅创建新文件：已存在返回 file_exists；父目录不存在返回 parent_not_found
  - edit_file 精确替换：old_string 不存在返回 no_match；返回 occurrences

### 2.3 错误模型（Phase 0 最低要求）

- 目的：为 CLI/E2E 提供可分类、可断言的失败语义，避免“随意字符串”导致回归脆弱
- Phase 0 允许以“约定字符串错误码”为过渡，但要求：
  - 错误码集合固定（如 file_not_found/parent_not_found/permission_denied/is_directory/invalid_path/file_exists/no_match/timeout/command_not_allowed）
  - CLI 不改写错误码语义
  - E2E 断言只依赖错误码与行为，不依赖底层错误消息细节

### 2.4 Tool 协议（CLI 驱动的稳定面）

- Tool 协议需满足：
  - `name`：稳定字符串
  - `call(input_json)`：输入为 JSON
  - `output_json`：输出为 JSON 值（数组/对象/字符串）

- Phase 0 约束：
  - CLI 的 `tool` 子命令必须做到“输入 JSON → 输出 JSON”，方便脚本化与 E2E
  - Tool 输出应尽量结构化（ls/grep/write/edit/execute/read_file）
    - 当前实现中 `read_file` 输出为结构化对象：`{ content, truncated, next_offset }`，其中 `content` 为 cat -n 风格文本，见 [std_tools.rs](../../crates/deepagents/src/tools/std_tools.rs#L66-L123)

### 2.5 本地 sandbox 安全边界（最低可用）

- 路径越界必须拒绝：
  - `../`、符号链接绕行（尽量做 canonicalize 后检查 starts_with(root)）
- execute 的默认策略：
  - Phase 0：库层可以允许执行（用于开发验证），但必须支持 allow-list 能力注入
  - Phase 0：必须具备 timeout，避免测试/CI 卡死

## 3. 详细迭代拆解（建议按里程碑推进）

### M0：Workspace 初始化与基础工程可用

- 任务
  - 初始化 workspace 与 crates 目录结构
  - 统一依赖与 edition
  - 提供最小 `cargo test` 可运行
- 验收
  - workspace 能 build
  - `cargo test` 通过（即使测试为空）

### M1：Core SDK 协议层（Backend/Tool/类型）

- 任务
  - 定义 backend trait（Filesystem/Sandbox）
  - 定义 tool trait 与 ToolResult
  - 定义跨层共享类型（FileInfo/GrepMatch/WriteResult/EditResult/ExecResult）
- 验收
  - Core crate 编译通过
  - 对外 API 与模块边界清晰（core 不依赖 CLI）
  - 第三方可替换性验证：用最小 mock backend（实现 trait）替换 LocalSandbox 通过编译并可被工具调用

### M2：LocalSandbox 实现（文件与搜索）

- 任务
  - 实现 ls/read/write/edit/glob/grep
  - 实现 root 边界与路径 normalize
  - 保证 read_file 行号输出与分页语义
- 验收
  - 单元测试覆盖：
    - 写入→读取→编辑→再读取
    - glob 命中与稳定排序
    - grep 字面量匹配与 line 1-based
  - 负向测试覆盖：
    - 读不存在文件
    - 写父目录不存在
    - edit no_match

### M3：LocalSandbox execute（timeout + allow-list 钩子）

- 任务
  - 实现 execute（stdout/stderr 合并输出）
  - 实现 timeout
  - 实现 allow-list 校验与危险 pattern 拒绝（至少覆盖最常见模式）
- 验收
  - 单元测试覆盖：
    - allow-list 允许的命令可执行
    - 非 allow-list 命令被拒绝（command_not_allowed）
    - timeout 生效（不会挂死）

### M4：默认工具集与 CLI tool 子命令闭环

- 任务
  - 提供默认工具集合（与 LocalSandbox 绑定）
  - CLI 增加 `tool` 子命令：输入 JSON → 输出 JSON
  - 定义输出格式的最小稳定性要求（例如 pretty/compact 两种输出）
- 验收
  - CLI 可驱动调用全部工具
  - CLI 对非法 JSON 与未知 tool 的错误提示可读、可断言
  - CLI 与 core 的耦合度可控：CLI 不直接依赖 LocalSandbox 类型，只依赖 `create_deep_agent` 或 `dyn SandboxBackend` 等 trait 边界

### M5：Phase 0 端到端验证与文档固化

- 任务
  - 固化技术设计（TECH_DESIGN）
  - 固化 E2E 计划（E2E_PHASE0）
  - 追加 “如何快速手动验证” 文档片段（面向开发者）
- 验收
  - 新同学按文档能复现：构建、运行、调用 2-3 个工具并看到预期效果

## 4. Phase 0 E2E 测试落地建议（从计划到可执行）

Phase 0 已有 E2E 测试计划文档；建议在 Phase 1 开始前，把 E2E 至少落地为“可执行的 smoke E2E”：

- 最小落地集合（建议 8-12 条）
  - CLI 启动与非法 JSON
  - ls/read_file（分页）
  - write_file/edit_file
  - glob/grep
  - execute（成功 + timeout）
- 执行入口建议
  - `crates/deepagents-cli/tests/e2e_cli.rs`：用 Rust 测试 spawn CLI 并断言 stdout JSON

## 5. 风险与回滚策略

- 风险：输出格式不稳定导致 E2E 易碎
  - 策略：把行号分隔符、关键字段名当作契约写入 TECH_DESIGN，并在测试中严格断言
- 风险：路径安全边界不足
  - 策略：Phase 0 单测必须包含越界用例；后续 Phase 2 再强化 CLI 产品层策略
- 风险：execute 造成 CI 不稳定
  - 策略：timeout 必须默认启用；输出截断必须可控
