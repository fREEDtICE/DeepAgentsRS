# Phase 3 E2E 测试计划（ACP Server：端到端会话与工具调用，黑盒）

适用范围：本计划面向 [ITERATION_PLAN.md](file:///Users/dafengxu/Dev/deepagents/rust/docs/ITERATION_PLAN.md#L131-L150) 的 Phase 3：提供可用的 ACP 服务端（会话、消息、工具调用、结果回传），并复用 Phase 1/2 的 tool schema 与错误码。**本计划为黑盒 E2E**：只关注外部可观察行为与结果，不依赖实现细节、内部模块、源码结构或运行时选型。

效果基准：体验对齐 Python 版本 ACP 的关键路径（“能建立会话、能调用工具、能拿到结构化结果、能关闭会话、错误语义稳定”），但不参考 Python 代码细节。

## 0. 术语与约束

- **ACP Server**：对外暴露网络接口的服务端，支持会话与工具调用。
- **Client**：测试驱动方（第三者视角），通过 HTTP/WebSocket/JSON-RPC/gRPC 等任意传输协议访问 Server（具体协议由 Phase 3 实现决定；本计划以“能力”定义用例）。
- **Session**：服务端维护的会话上下文，至少能关联 state（如 FilesystemState）与权限策略（Phase 2）。
- **Tool**：Phase 1/2 已定义的工具集（至少：`read_file`、`grep`、`execute`；可包含 `ls/glob/write_file/edit_file/delete_file`）。
- **Root**：Sandbox 根目录边界（客户端可能通过创建会话时指定；若不支持则由服务端配置）。

黑盒测试原则：

- 不假设任何内部实现（例如 tokio、数据库、serde 结构等）
- 只断言：请求/响应、状态变化、错误码语义、时序与可重复性
- 如果协议细节未最终确定，E2E 用“兼容断言”描述（例如允许字段名别名、或允许 REST/JSON-RPC 两套入口任选其一），但必须固定最小可测契约点

## 1. 完成定义（E2E 角度）

Phase 3 E2E 通过必须满足：

- 服务器可启动并对外提供服务（可在本地/CI 重复执行）
- 覆盖核心用例组：
  - 会话生命周期：建立 → 使用 → 关闭（含幂等与资源回收）
  - 工具调用：请求 → 执行 → 返回结构化结果（含错误）
  - state 可观测与持续演进（至少 FilesystemState）
  - 复用 Phase 1/2 契约：tool schema 与错误码语义稳定
  - 安全策略：execute deny-by-default/allow-list/危险模式拒绝可回归
- 所有用例使用隔离 root（临时目录），无副作用（不会泄露 root 外内容/不会污染宿主环境）

## 2. 测试入口与 Harness（第三者视角）

### 2.1 推荐 E2E Harness（协议无关）

建议使用“黑盒驱动器”以最小依赖编写：

- 启动服务端进程（固定端口或动态端口）
- 通过网络请求调用“会话 API / 工具调用 API”
- 用临时目录作为 root，写入 fixture 文件
- 断言响应 JSON 与副作用（文件系统、state）

### 2.2 必须固化的最小外部契约点（否则 E2E 无法落地）

无论最终采用何种协议，必须提供等价能力：

1) **创建会话**  
输入：root（或 server-side profile）、可选的执行策略（allow-list/非交互）  
输出：`session_id`（稳定可用的标识符）

2) **调用工具（会话内）**  
输入：`session_id`、`tool_name`、`input`（JSON）  
输出：结构化结果，至少包含：
- `output`（工具输出）
- `error`（可空；失败时含错误码/原因）
- `state` 或 `delta`（至少其中之一可用于验证 state 演进；建议两者都有）

3) **读取会话 state（可选但强烈建议）**  
输入：`session_id`  
输出：完整 state（至少包含 filesystem files 快照）

4) **关闭会话**  
输入：`session_id`  
输出：成功/失败（幂等）

备注：如果 Phase 3 决定不提供“读取 state”接口，则必须在 tool 调用响应中返回 `state`（或可查询的 state 版本号 + delta），否则无法黑盒断言 state 演进。

### 2.3 默认协议（HTTP + JSON，v1）

为保证 E2E 可直接落地，Phase 3 默认提供一套 HTTP + JSON 的对外契约。该契约是“可观察行为”的一部分；未来如增加 stdio/JSON-RPC 等传输，需保证语义等价并复用同一套 E2E 断言。

**固定字段**

- 所有响应都包含：
  - `protocol_version: "v1"`
  - `ok: boolean`
  - `ok=true` 时包含 `result`
  - `ok=false` 时包含 `error: { code, message, details? }`

**端点**

- `POST /initialize`
  - 响应 `result`：包含 server 名称、协议版本、能力声明（例如 supports_state）
- `POST /new_session`
  - 请求：`{ root, execution_mode?, shell_allow_list?, audit_json?, protocol_version? }`
  - 响应：`{ session_id }`
- `POST /call_tool`
  - 请求：`{ session_id, tool_name, input, protocol_version? }`
  - 响应：`result` 至少包含：
    - `output: any|null`
    - `error: { code, message } | null`（工具级错误）
    - `state` 与/或 `delta`
    - `state_version: number`
- `GET /session_state/{session_id}`
  - 响应：`{ state, state_version }`
- `POST /end_session`
  - 请求：`{ session_id, protocol_version? }`
  - 响应：`{ already_closed: boolean }`（幂等）

## 3. Phase 3 统一 Fixture（所有用例复用）

每个用例创建独立 root（临时目录），预置：

- `README.md`：包含 `needle` 两次
- `src/lib.rs`：包含 `needle` 一次
- `empty.txt`：空文件
- `large.txt`：> 500 行（分页/截断验证）
- `secret.txt`：包含敏感文本（用于越界/泄露测试，仅在 root 内）

可选（安全回归用）：

- `outside_secret.txt`：root 外的敏感文件（用于越界访问拒绝）
- `link_to_outside`：root 内符号链接指向 root 外（若系统支持）

## 4. E2E 用例清单（按能力域分组）

### 4.1 服务端启动与健康检查

**E2E-ACP-BOOT-001：服务可启动并监听端口**

- 步骤：启动 server（配置 bind 地址），等待就绪（health 或首个请求成功）
- 期望：可建立连接；无崩溃；启动日志不包含敏感信息
- 断言：返回 200/OK 或等价成功信号；失败时错误可诊断

**E2E-ACP-BOOT-002：并发连接基本可用**

- 步骤：并发建立 N 个会话（例如 10），各自执行 read_file
- 期望：无死锁/挂起；响应在合理时间内返回
- 断言：每个会话独立成功，或在资源限制下给出明确错误码（如 rate_limited）

### 4.2 会话生命周期（核心）

**E2E-ACP-SESSION-001：创建会话返回 session_id**

- 输入：root +（可选）执行策略
- 期望：返回唯一 session_id
- 断言：session_id 非空、格式稳定（可作为路径参数/JSON 字段）

**E2E-ACP-SESSION-002：关闭会话幂等**

- 步骤：end(session_id) 两次
- 期望：第一次成功，第二次仍返回“已关闭”或成功（幂等）
- 断言：不返回 500；错误码语义稳定（如 session_not_found 或 already_closed）

**E2E-ACP-SESSION-003：关闭后不可再调用工具**

- 步骤：end(session_id) 后 tool_call
- 期望：拒绝
- 断言：错误码为 session_not_found/already_closed（选其一并固定）

**E2E-ACP-SESSION-004：无效 session_id 拒绝**

- 输入：随机 session_id 调用 tool
- 期望：拒绝
- 断言：错误码稳定（session_not_found）

### 4.3 工具调用：成功路径（核心）

**E2E-ACP-TOOL-READ-001：read_file 成功返回结构化输出**

- 步骤：创建会话 → 调用 read_file(README, offset/limit) → 校验
- 期望：返回 content/行号（或等价表示），且可分页
- 断言：
  - 输出包含 README 的前几行（或包含 line-number 风格）
  - 若有 truncation 信号，则字段/语义稳定（truncated/next_offset 等）

**E2E-ACP-TOOL-GREP-001：grep content 输出结构化且 line 为 1-based**

- 步骤：grep needle（output_mode=content）
- 期望：返回数组，元素含 path/line/text
- 断言：line 从 1 开始；path 可用于 read_file

**E2E-ACP-TOOL-GREP-002：grep output_mode 一致性**

- 断言：
  - files_with_matches 覆盖 content 的 path 集合
  - count 与 content 的数量逻辑一致（考虑 head_limit 截断）

**E2E-ACP-TOOL-EXEC-ALLOW-001：execute allow-list 放行**

- 预置：会话创建时配置 allow-list（例如允许 echo/sleep）
- 步骤：execute("echo hello")
- 期望：exit_code=0，output 包含 hello
- 断言：不会泄露 root 外信息；输出截断字段存在且语义稳定

### 4.4 工具调用：错误语义与 schema（核心）

**E2E-ACP-SCHEMA-001：工具入参 schema 严格校验**

- 对每个工具构造：缺必填字段、错类型、未知字段（若 strict）
- 断言：返回可分类 schema 错误（schema_validation_failed/invalid_input 等）；无 panic；错误包含字段名

**E2E-ACP-ERR-READ-001：file_not_found**

- read_file 不存在文件
- 断言：错误码 file_not_found；不会返回 500

**E2E-ACP-ERR-READ-002：is_directory**

- read_file 传入目录
- 断言：错误码 is_directory

**E2E-ACP-ERR-EDIT-001：no_match**

- edit_file old_string 不存在
- 断言：error=no_match；occurrences=0 或缺省（契约固定其一）

**E2E-ACP-ERR-EXEC-001：deny-by-default**

- 未配置 allow-list 或明确 deny
- execute 任意命令
- 断言：command_not_allowed 或 approval_required（契约固定）；不执行命令

**E2E-ACP-ERR-EXEC-002：危险 pattern 拒绝**

- 输入包含危险模式（如 `$(...)`、重定向 `>`、单独 `&`）
- 断言：拒绝且错误码可分类（dangerous_pattern 或 command_not_allowed + reason）

**E2E-ACP-ERR-EXEC-003：timeout**

- execute("sleep 2", timeout=1)
- 断言：timeout；服务端不挂死；资源可回收

### 4.5 State 演进与可观测（核心）

**E2E-ACP-STATE-001：write_file 产生 state 快照**

- write_file 创建 a.txt
- 断言：state（或 delta）中出现该文件条目；path/内容字段满足最小契约

**E2E-ACP-STATE-002：edit_file 更新 state**

- edit_file 替换一次
- 断言：state 内容更新；modified_at 或 delta 有变化（契约固定其一）

**E2E-ACP-STATE-003：delete_file 删除语义**

- delete_file 删除 a.txt
- 断言：state 中该条目移除或标记 deleted=true（需与 Phase 1 删除语义一致）

**E2E-ACP-STATE-004：同会话多次调用 state 连续演进**

- S1：write → edit → read → grep
- 断言：每一步的 state 版本单调演进；不会回退或丢失条目

### 4.6 多会话隔离与并发（核心）

**E2E-ACP-ISO-001：会话间 filesystem state 隔离**

- session A 写 a.txt
- session B 读取 a.txt（同 root 或不同 root，按产品设计）
- 期望：按契约：
  - 若 root 隔离：B 不可见 A 的文件
  - 若共享 root：文件可见，但 state 应独立（B 的 state 不应自动出现 A 的快照，除非 B 也触发工具）
- 断言：选择其一并固化为协议文档；测试覆盖该语义

**E2E-ACP-ISO-002：并发 tool 调用不互相污染**

- 同一 session 并发发起两个 read_file（不同文件）
- 断言：响应可对应请求；state 合并无丢失

### 4.7 传输层鲁棒性（黑盒回归）

**E2E-ACP-NET-001：请求体过大被拒绝**

- 发送超大 input（例如 write_file content 超大）
- 断言：返回 payload_too_large 或等价错误；不崩溃

**E2E-ACP-NET-002：非法 JSON/协议帧**

- 发送非法 JSON
- 断言：返回 parse_error；不崩溃

### 4.8 安全边界（必须）

**E2E-ACP-SEC-001：路径越界拒绝（../）**

- read_file("../outside_secret.txt")
- 断言：permission_denied/invalid_path；不泄露内容

**E2E-ACP-SEC-002：符号链接绕行拒绝（若纳入）**

- root 内创建 symlink 指向 root 外文件 → read_file
- 断言：拒绝；不泄露

**E2E-ACP-SEC-003：审计输出不泄露敏感信息（若 Phase 2 审计纳入 server）**

- execute 带疑似 secret 参数（如 `--token abc`）
- 断言：审计输出（若启用）不包含明文 secret

## 5. 结果断言规范（黑盒一致性）

为确保不同实现（Rust/Python、不同传输层）仍可复用测试，建议 E2E 统一断言以下语义字段：

- 成功响应必须能取到 `output`
- 失败响应必须能取到 `error`（包含可分类 code 或可匹配的错误码字符串）
- state 必须可观测（响应携带 state 或提供独立 state API）
- 工具输出必须与 Phase 1/2 schema 对齐（字段名、默认值、1-based line、truncation 信号等）

如果协议采用 JSON-RPC，建议把错误码映射到 JSON-RPC error.code，并在 error.message 中保留项目错误码（如 `file_not_found`）。

## 6. 落地建议（从计划到可执行）

- 文档：本文件作为 Phase 3 ACP 黑盒 E2E 基线
- 测试工程建议（二选一或两者并存）：
  - A：`crates/deepagents-acp/tests/e2e_acp_blackbox.rs`（启动 server 进程 + 网络请求）
  - B：仓库根 `tests/acp_e2e/`（用脚本语言驱动，适合跨协议）
- 用例可分级：
  - smoke：BOOT/SESSION/READ/GREP/EXEC deny-by-default
  - full：全部用例组（并发/鲁棒/安全）
- CI 建议：
  - 端口冲突处理（动态端口或随机端口）
  - 用例并行需确保 root 隔离与 session 隔离
