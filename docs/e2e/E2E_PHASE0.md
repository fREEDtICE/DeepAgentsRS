# Phase 0 E2E 测试计划（最小工程骨架 + 本地工具闭环）

目标：验证 Rust 版在 Phase 0 能够通过 CLI 驱动工具，完成文件系统探索、读取、写入、编辑、搜索与命令执行，并在错误/边界场景下给出可预期、稳定、可自动化断言的结果。效果可参考 Python 版本的用户体验层面，但不依赖其代码实现细节。

## 1. 范围与不范围

- 范围（Phase 0 必测）
  - workspace 可构建、可运行、可执行 E2E 测试套件
  - CLI 驱动工具调用闭环（tool 名称、JSON 输入、JSON 输出）
  - 工具能力：`ls` / `read_file` / `write_file` / `edit_file` / `glob` / `grep` / `execute`
  - 关键行为特征
    - `read_file` 输出带行号（cat -n 风格）
    - `grep` 为字面量匹配、返回结构化匹配信息
    - `execute` 返回 exit code + 输出（stdout/stderr 合并）

- 不范围（Phase 0 不要求，但要留钩子）
  - 完整 Agent runtime（模型推理、自动 tool call 规划）
  - Middleware 状态回填/Reducer（Phase 1）
  - ACP server 端到端会话（Phase 3）
  - Skills 插件（Phase 6）

## 2. 测试策略与组织方式

- 测试类型
  - 黑盒 E2E：以 CLI 作为唯一入口，工具调用通过命令行 JSON 入参触发，stdout/stderr 用于断言
  - 负向/安全：非法路径、越界路径、不可用命令、危险 pattern、超时等

- 断言原则
  - 只断言行为与结构，不断言内部实现
  - 对齐效果：输出格式、错误可读性、分页、排序稳定性
  - 确定性：每个测试用例自建测试根目录与固定文件集，避免依赖宿主机器状态
  - Trait 优先的可替换性：E2E 只通过 CLI 验证用户可见行为；“第三方替换 backend/tool” 的验证在单测/集成测试中通过替换 trait 实现完成（Phase 0 至少要求具备该验证入口）

- E2E Harness 建议
  - 用 Rust 集成测试 spawn CLI（推荐）
  - 驱动方式统一：
    - `deepagents --root <root> tool <name> --input '<json>' [--pretty]`
  - 输出解析：
    - stdout 解析为 JSON（工具输出为 JSON 值）
    - 若为字符串类输出（如 read_file），仍以 JSON string 方式解析并断言内容

## 3. 测试环境与前置条件

- 环境
  - macOS/Linux（Phase 0 以类 Unix 为主）
  - 需要 `sh` 可用（用于 execute）
  - 测试不依赖网络

- 构建与运行
  - `cargo build`
  - E2E 测试入口建议：`cargo test -p deepagents-cli`（或单独 e2e crate）

- 数据隔离
  - 每个用例创建独立 root（临时目录）
  - 禁止使用真实工作区作为 root

## 4. 统一测试数据集（Fixture）

每个用例 root 下创建如下结构（可按需裁剪）：

- `README.md`（多行文本，包含关键字 `needle`）
- `src/lib.rs`
- `src/nested/deep.txt`
- `empty.txt`（空文件）
- `unicode.txt`（含中文与特殊空白字符）
- `large.txt`（> 300 行，用于分页与边界）

内容约束（效果层面）：

- `README.md` 至少 5 行，其中 2 行包含 `needle`
- `unicode.txt` 包含 `中文 needle` 与 `tab\tneedle`

## 5. E2E 用例清单（按能力域分组）

### 5.1 CLI 基础与协议

**E2E-CLI-001：CLI 能启动并返回成功**

- 步骤：运行 `deepagents --help`
- 期望：退出码 0；输出包含 `tool` 子命令使用说明

**E2E-CLI-002：未知 tool 名称返回可理解错误**

- 步骤：`deepagents --root <root> tool not_a_tool --input '{}'`
- 期望：非 0 退出码；错误信息包含 “unknown tool” 或等价可读提示

**E2E-CLI-003：非法 JSON 输入被拒绝**

- 步骤：`--input '{bad json'`
- 期望：非 0；错误指明 JSON 无效

### 5.2 ls（目录列举）

**E2E-LS-001：列出根目录文件**

- 步骤：构建 fixture；`tool ls --input '{"path":"<root>"}'`
- 期望：输出为 JSON 数组；包含 fixture 顶层条目
- 断言：每个条目至少包含 `path`；路径为绝对路径；排序稳定（可排序后集合相等）

**E2E-LS-002：ls 输入非目录路径的错误行为**

- 步骤：`tool ls --input '{"path":"<root>/README.md"}'`
- 期望：失败（非 0 或结构化错误）；错误可分类（如 `invalid_path`/“not a directory”）

### 5.3 read_file（分页读取 + 行号格式）

**E2E-READ-001：读取文件前 3 行并带行号**

- 步骤：`tool read_file --input '{"file_path":"<root>/README.md","limit":3}'`
- 期望：输出包含 3 行；行号从 1 开始
- 断言：包含稳定行号分隔符；每行包含原始文本片段

**E2E-READ-002：分页 offset 生效**

- 步骤：
  - `limit=2, offset=0` 获取前两行
  - `limit=2, offset=2` 获取第 3-4 行
- 期望：两次输出不重叠且拼接覆盖前 4 行
- 断言：第二次输出行号从 3 开始

**E2E-READ-003：读取空文件返回可检测提示**

- 步骤：`tool read_file --input '{"file_path":"<root>/empty.txt","limit":50}'`
- 期望：输出包含可检测的空文件提示文本

**E2E-READ-004：读取不存在文件**

- 步骤：`tool read_file --input '{"file_path":"<root>/nope.txt"}'`
- 期望：失败并返回可分类错误（如 `file_not_found`）

### 5.4 write_file（仅创建新文件）

**E2E-WRITE-001：写入新文件成功**

- 步骤：
  - `tool write_file --input '{"file_path":"<root>/new.txt","content":"hello\n"}'`
  - `read_file` 验证
- 期望：write 返回成功结构（至少含 path）；read 可读到内容（带行号）

**E2E-WRITE-002：写已存在文件被拒绝**

- 步骤：对同一路径重复 write
- 期望：返回 `file_exists`；原内容不变

**E2E-WRITE-003：父目录不存在**

- 步骤：`file_path="<root>/missing_dir/a.txt"`
- 期望：返回 `parent_not_found`；目录不被隐式创建

### 5.5 edit_file（精确替换）

**E2E-EDIT-001：替换成功并返回 occurrences**

- 步骤：写入包含 `OLD` 的文件；edit `OLD→NEW`；read 验证
- 期望：occurrences=出现次数；仅精确替换，不做 regex

**E2E-EDIT-002：old_string 不存在返回 no_match**

- 步骤：对不包含 old_string 的文件 edit
- 期望：返回 `no_match`；文件内容不变

**E2E-EDIT-003：编辑不存在文件**

- 步骤：`edit_file` 指向不存在路径
- 期望：`file_not_found`

### 5.6 glob（递归匹配）

**E2E-GLOB-001：递归 glob 匹配**

- 步骤：fixture 中存在 `src/nested/deep.txt`；`tool glob --input '{"pattern":"**/*.txt"}'`
- 期望：返回绝对路径数组，包含相关 `.txt`
- 断言：排序稳定；不包含目录

**E2E-GLOB-002：空 pattern 或非法 pattern 处理**

- 步骤：pattern 为空
- 期望：失败并返回可理解错误

### 5.7 grep（字面量匹配）

**E2E-GREP-001：grep 返回结构化匹配**

- 步骤：`tool grep --input '{"pattern":"needle","path":"<root>","glob":"**/*.md","output_mode":"content","head_limit":50}'`
- 期望：返回匹配列表
- 断言：每条包含 `path`（绝对）、`line`（1-based）、`text`；`text` 包含 `needle`

**E2E-GREP-002：grep 字面量（非 regex）**

- 步骤：写入包含 `a.b` 的行；pattern=`a.b`
- 期望：能匹配到 `a.b`

**E2E-GREP-003：output_mode 行为**

- 步骤：分别用 `files_with_matches` / `count` / `content`
- 期望：三者输出形态正确，且数量逻辑一致

### 5.8 execute（命令执行 + 超时 + 安全）

**E2E-EXEC-000：后端不支持 execute 时返回稳定错误**

- 前置：在测试环境中选择一个不支持命令执行的后端/运行模式（或显式关闭 execute 能力）
- 步骤：`tool execute --input '{"command":"pwd","timeout":5}'`
- 期望：失败但不崩溃；返回可理解、可脚本化断言的错误
- 断言：错误可分类（如 `not_supported`/`execute_not_supported`）；退出码非 0；不会真正执行命令

**E2E-EXEC-001：执行简单命令成功**

- 步骤：`tool execute --input '{"command":"pwd","timeout":5}'`
- 期望：exit_code=0；output 非空（可包含 root）

**E2E-EXEC-002：非 0 exit code 透传**

- 步骤：执行明确失败的命令
- 期望：exit_code 非 0；output 包含错误信息（若有）

**E2E-EXEC-003：超时生效**

- 步骤：执行长命令；timeout=1
- 期望：返回 `timeout` 或等价可分类失败；不会挂起

**E2E-EXEC-004：危险 pattern 拒绝（预留，Phase 2 强制）**

- 前置：启用 CLI allow-list/审批策略后执行
- 步骤：命令包含 `$(`、`>`、`<<`、裸变量 `$HOME`、单独 `&` 等
- 期望：返回 `command_not_allowed`（执行前拒绝，无副作用）

### 5.9 安全：路径校验（对齐 Python validate_path 语义）

说明：本组用例用于把“路径字符串的拒绝规则”固化为可回归契约（避免不同平台/实现细节漂移）。所有工具只要接收路径参数（如 `path/file_path`），都应满足等价拒绝规则；Phase 0 以 `read_file`/`ls` 为代表做黑盒回归即可。

**E2E-PATH-001：拒绝包含 `..` 的路径段**

- 步骤：`tool read_file --input '{"file_path":"<root>/a/../README.md","limit":3}'` 或 `{"file_path":"../README.md"}`
- 期望：失败
- 断言：错误可分类（如 `invalid_path` 或 `permission_denied`，二选一并固定）

**E2E-PATH-002：拒绝 `~` 开头路径**

- 步骤：`tool read_file --input '{"file_path":"~/secret.txt"}'`
- 期望：失败
- 断言：错误可分类为 `invalid_path`（或固定枚举）；不泄露宿主用户目录内容

**E2E-PATH-003：拒绝 Windows 盘符绝对路径**

- 步骤：`tool read_file --input '{"file_path":"C:\\\\Windows\\\\win.ini"}'`
- 期望：失败
- 断言：错误可分类为 `invalid_path`（或固定枚举）；不泄露宿主内容

**E2E-PATH-004：normpath 归一化后仍越界必须拒绝**

- 步骤：构造带多余分隔符/相对段的路径，例如 `"<root>//a//..//../outside.txt"`
- 期望：失败
- 断言：错误可分类；不会“自动修正”为 root 内其他文件

## 6. 覆盖矩阵（用例 → 能力）

- CLI：CLI-001/002/003
- ls：LS-001/002
- read_file：READ-001/002/003/004
- write_file：WRITE-001/002/003
- edit_file：EDIT-001/002/003
- glob：GLOB-001/002
- grep：GREP-001/002/003
- execute：EXEC-001/002/003；EXEC-004（预留，Phase 2 强制）

## 7. 风险点与特殊说明

- 输出格式稳定性：`read_file` 的行号分隔符与行号语义要作为契约，避免 E2E 漂移
- 文件系统边界：必须包含路径校验负例（`..`、`~`、Windows 盘符、归一化越界），验证 root 约束不会被绕过
- execute 能力门禁：execute 可能因后端/运行模式不可用；不支持时应返回稳定错误而非崩溃
- 安全模式开关：Phase 0 可先验证 execute 基本行为与超时，但危险模式拒绝用例要预留并在 Phase 2 变为强制回归
