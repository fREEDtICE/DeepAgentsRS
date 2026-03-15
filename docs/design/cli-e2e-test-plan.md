# DeepAgents CLI E2E 测试方案（模块化）

## 1. 目标与范围

本方案面向 `crates/deepagents-cli` 的黑盒端到端（E2E）测试，目标是：
- 以真实 CLI 入口验证用户可见行为与跨模块集成行为。
- 以稳定、可脚本化、可并行的方式覆盖关键发布契约。
- 通过 Mock Provider 与本地假服务减少外部依赖，提升可重复性。

范围包含：
- 命令面：`tool / run / skill / config / memory`
- 运行面：runtime 中间件装配链与状态恢复链
- 提供方：`mock/mock2/openai-compatible`
- 可观测性：events、audit、trace、错误分类

不包含：
- 纯单元测试（由各 crate 内部单测负责）
- 基准性能测试
- 真实线上模型可用性探测

## 2. 现状基线（从源码与文档提炼）

当前已具备较完整 CLI E2E 覆盖，主要集中在 `crates/deepagents-cli/tests`，文件包括：
- `e2e_cli.rs`（基础 tool 命令链）
- `e2e_phase1_5_runtime.rs`（run 主循环与错误分类）
- `e2e_phase2_security.rs`（安全边界）
- `e2e_phase6_skills.rs`、`e2e_phase7_skill_registry.rs`（skills 生命周期与选择）
- `e2e_openai_compatible.rs`（OpenAI 兼容 provider 协议）
- `e2e_runner_events.rs`、`e2e_prompt_caching.rs`（观测与缓存）
- `e2e_config.rs`、`e2e_phase1_stateful.rs`、`e2e_phase4_subagents.rs`

可复用测试模式已形成：
- `Command::new(env!("CARGO_BIN_EXE_deepagents"))` 黑盒执行
- `tempfile::tempdir()` 动态 fixture
- `--provider mock|mock2 --mock-script script.json` 行为可控
- 对 `stdout JSON + stderr + 落盘文件(events/audit/state)` 组合断言

## 3. 测试分层策略

### 3.1 L0 冒烟层（PR 必跑）

目标：最短反馈链路，确认 CLI 主路径可用。

建议覆盖：
- `tool ls/read_file/write_file/edit_file`
- `run --provider mock`（1 次 tool 调用 + final_text）
- `config set/get`（含 secret redaction）
- `skill install/status`（最小生命周期）

通过标准：
- 所有用例 < 2 分钟总时长（按 CI 能力调节）
- 无网络依赖、无随机失败

### 3.2 L1 主契约层（PR/主干）

目标：保护关键行为协议，阻断回归。

建议覆盖：
- runtime 错误分类：`provider_timeout / max_steps_exceeded`
- security：路径逃逸、软链逃逸、命令白名单
- skills：resolve 选择与 skipped reason、quarantine/enable block
- stateful：`run -> resume` 状态连续性
- events/audit：关键字段存在与语义正确

### 3.3 L2 扩展层（夜间）

目标：高成本场景与组合场景完整性。

建议覆盖：
- openai-compatible 流式 + structured output
- prompt caching 事件与敏感信息脱敏
- subagent/skill isolation capsule
- 多中间件叠加场景（skills + todo + memory + patch/tool compat）

## 4. 模块化测试矩阵

## 4.1 CLI 基础命令模块（tool）

关注点：
- 文件系统工具链行为是否与用户输入一致
- JSON 输出结构稳定（可被上层消费）

核心用例：
- 成功路径：`ls -> read_file`、`write_file -> edit_file -> delete_file`
- 失败路径：文件不存在、目录误读、schema 不匹配
- 安全路径：根目录外访问拒绝、软链越界拒绝

关键断言：
- `status.success()`
- `tool_results[*].output/error`
- 文件最终存在性与内容一致性

## 4.2 Runtime 主循环模块（run + provider mock）

关注点：
- 多轮 tool call 与最终输出拼装
- 错误分类与退出码一致性

核心用例：
- 单轮/多轮 tool calls
- 无 tool 直接 final
- unknown tool、tool error 不阻断 final（可配置语义）
- provider timeout、max steps 上限触发

关键断言：
- `final_text/tool_calls/tool_results/trace/error.code`
- 成功/失败退出码与 `error.code` 对齐

## 4.3 状态与恢复模块（stateful/resume）

关注点：
- thread/state 文件写入与恢复正确性
- refresh 行为是否真正刷新 snapshot

核心用例：
- 首次 run 产生 thread_id/state
- resume 后行为连续
- `--refresh-skill-snapshot` 触发新快照生效

关键断言：
- `state.extra.thread_id`
- refresh 前后 selected skills 变化
- audit 可按 thread_id 回放记录

## 4.4 Skill 注册与治理模块（skill）

关注点：
- install/status/versions/enable/disable/remove 生命周期
- resolve 打分、selected/skipped 可解释性
- 治理规则（quarantine/governance）不可绕过

核心用例：
- 同名多版本并存与版本查询
- 关键字/契约触发选择
- quarantined skill 被跳过且无法 enable
- manual selection 与 run 集成

关键断言：
- `summary/installed/changed/removed`
- `snapshot.selection.selected/skipped/reasons`
- 错误码 `governance_blocked`

## 4.5 配置与记忆模块（config + memory）

关注点：
- workspace/global 配置读取优先级
- 密钥状态可见但值不可泄漏
- memory store 策略与 runtime_mode 配置生效
- memory 生命周期命令与跨作用域访问控制

核心用例：
- `config set/get/doctor` 联动
- run 读取 workspace provider 默认参数
- memory `put/remember/get/explain/edit/pin/unpin/delete/query/compact` 全链路
- actor 上下文（user/thread/workspace）下的读写权限与可见性
- `memory.file.eviction/ttl/max_entries/max_bytes_total/runtime_mode` 覆盖

关键断言：
- `secret_status == set` 且 stdout 不含明文 key
- provider 请求参数符合配置
- store 文件实际落盘路径正确
- scoped/compatibility 运行模式注入字段符合预期

## 4.6 Provider 协议兼容模块（openai-compatible）

关注点：
- 请求体协议、流式协议、结构化输出协议

核心用例：
- fake axum server 校验 `/chat/completions` body
- SSE 流式事件被 runtime 正确消费
- structured output schema 下 JSON 结果解析

关键断言：
- 请求 `model/messages/response_format` 字段
- stderr 能力打印（stream-events）关键能力位
- `final_text` 与 `structured_output` 一致

## 4.7 可观测性模块（events/audit/prompt-cache）

关注点：
- 关键运行事件可追踪
- 敏感输入不出现在日志/trace

核心用例：
- `--events-jsonl` 输出结构校验
- `skill audit` 与 run trace 对齐
- prompt cache 事件生成与 secret 脱敏

关键断言：
- event type 序列、字段完整性
- 审计记录 thread_id 对齐
- stdout/stderr 不包含敏感字串

## 5. Fixture 与数据组织规范

统一建议：
- 每个测试独立 `tempdir`，避免共享状态。
- 输入工件统一放测试根目录：
  - `script.json`（mock provider steps）
  - `skills/<name>/SKILL.md` 与 `tools.json`
  - `state/thread.json`、`events.jsonl`、`audit.json`
- helper 函数统一：
  - `run_cli(root, args) -> (status, json, stderr)`
  - `write_json(path, value)`
  - `write_skill_package(...)`

数据设计原则：
- 最小化脚本（每例仅保留必要 steps）
- 显式 call_id（便于追踪）
- 错误场景优先断言 `error.code` 或稳定关键片段，降低文案变化导致的脆弱性

## 6. CI 执行与分组建议

建议分组（以 test 文件名/标签组织）：
- `e2e_smoke`：L0 用例
- `e2e_contract`：L1 用例
- `e2e_extended`：L2 用例

建议执行策略：
- PR：`smoke + contract`
- 夜间：`smoke + contract + extended`
- 回归排查：支持单文件/单用例精确运行

稳定性建议：
- 全部本地 fake 服务，不依赖公网
- 等待点使用短固定 sleep + 明确超时
- 禁用不必要并发共享资源

## 7. 增量落地计划（模块逐步推进）

### 阶段 A：收敛基线
- 整理当前已有 E2E 到 L0/L1/L2 分组清单
- 提取公共 helper，减少重复样板代码

### 阶段 B：补齐薄弱面
- 补全 config 与 memory 交叉场景（优先级覆盖）
- 增加 subagent + skills + approval 联动边界

### 阶段 C：质量门禁
- 将 L0/L1 接入 PR 必跑
- 为关键错误码建立“回归白名单”断言集

### 阶段 D：可维护性优化
- 增加测试命名规范与失败信息模板
- 周期性清理脆弱断言（过度依赖完整文本）

## 8. 验收标准

方案完成后，以如下标准验收：
- 每个命令域（tool/run/skill/config/memory）至少 1 条成功 + 1 条失败 E2E。
- 每个核心错误分类（timeout/max_steps/security/治理阻断）都有稳定断言。
- 至少 1 条用例验证 secret 不泄露。
- 至少 1 条用例验证状态恢复链（run/resume/审计）闭环。
- CI 中 PR 路径可稳定运行，无外部网络依赖。

## 9. 建议的后续文档与实现同步

当新增 CLI 参数或 runtime 中间件时，同步更新：
- 本文“模块化矩阵”对应条目
- `crates/deepagents-cli/tests` 中对应分组用例
- RFC 中发布契约相关章节（若属于公开行为变更）

## 10. 逐命令逐参数 E2E 测试设计（补充）

本节给出“命令 -> 参数 -> 用例设计”的细化方案，目标是保证每个 CLI 参数至少有一条可执行的黑盒验证路径。

### 10.1 顶层参数（所有子命令共享）

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--root` | 指向临时目录；指向不存在目录 | 成功时读写根路径生效；失败时错误码/错误信息稳定 |
| `--shell-allow` | 传单值、重复多值、重复项去重 | `execute` 仅允许白名单命令；重复项不影响行为 |
| `--shell-allow-file` | 文件含空行/注释/重复 | 解析后白名单与预期一致；无效文件路径报错 |
| `--execution-mode` | `interactive/non-interactive` + 非法值 | 交互模式触发 HITL 中断默认集合；非法值被配置层拒绝 |
| `--audit-json` | 指定审计落盘路径 | 运行后 JSONL 存在且行可解析 |

### 10.2 `tool` 命令

命令：`deepagents tool <name> --input <json> [--pretty] [--state-file <path>]`

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `name` | 已存在工具名；不存在工具名 | 成功返回 output；不存在时报 unknown tool |
| `--input` | 合法 JSON；非法 JSON；缺字段 | schema 通过时成功；失败时错误稳定（missing/invalid） |
| `--pretty` | 开/关 | 仅格式化差异，不影响语义字段 |
| `--state-file` | 首次写入；重复调用复用状态 | 文件创建成功；二次调用读取到之前状态 |

### 10.3 `run` 命令（按参数域分组）

命令：`deepagents run --input <text> [大量可选参数]`

#### 10.3.1 Provider 与请求参数

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--provider` | `mock/mock2/openai-compatible/openrouter/unknown` | 已支持 provider 成功；unknown 失败并带稳定错误 |
| `--mock-script` | provider=mock/mock2 时缺失或提供 | 缺失时报 `--mock-script is required`；提供后按脚本执行 |
| `--model` | openai-compatible/openrouter 缺失或设置 | 缺失报 `--model is required`；设置后请求体 model 正确 |
| `--base-url` | 覆盖默认地址 | 实际请求发送到 fake server 地址 |
| `--api-key` | 直接传 key | 请求头带认证且不泄漏到 stdout |
| `--api-key-env` | 环境变量存在/不存在 | 存在时成功；不存在报 `missing env var for api key` |
| `--tool-choice` | `auto/none/required/named:x/named:`/非法值 | 合法值生效；`named:` 空名和非法值报稳定错误 |
| `--structured-output-schema` | 直接 JSON；`@file`；非法 JSON | 合法时输出 `structured_output`；非法 JSON 报错 |
| `--structured-output-name` | 未传/空值/自定义 | 未传时默认 `structured_output`；自定义名写入请求 |
| `--structured-output-description` | 传/不传 | 请求中 description 与入参一致 |

#### 10.3.2 状态与恢复参数

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--thread-id` | 显式 thread_id 与自动生成 | 显式值持久化到 `state.extra.thread_id` |
| `--state-file` | 首次 run + 二次 run/resume | 状态文件可读写；第二次读取后行为连续 |

#### 10.3.3 Skills 运行时参数

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--skills-source` | 单源/多源/坏源 | 正常源加载成功；坏源在 strict 下失败 |
| `--skill-registry` | 指定 registry；默认 registry | 解析来源符合优先级与路径 |
| `--skill` | `name` 与 `name@version` | 显式技能被 pin，出现在 selected |
| `--disable-skill` | 关闭已存在技能 | disabled 生效，不进入 selected |
| `--skill-select` | `auto/manual/off/非法值` | 合法值语义正确；非法值报 `invalid_arguments` |
| `--skill-max-active` | `0/1/N` | 运行态上限生效；resolve 分支验证 `0` 被钳制为 `1` |
| `--explain-skills` | 开关 | stderr 出现 skills 选择事件 |
| `--refresh-skill-snapshot` | 关闭/开启 | 开启后快照重算，selected 可变化 |
| `--skills-skip-invalid` | 开关 | 开启后坏包跳过；关闭时失败 |

#### 10.3.4 Memory 注入参数

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--memory-source` | 单源/多源/不存在 | 有效源被注入；不存在源在 strict 配置下失败 |
| `--memory-allow-host-paths` | 开关 | host path 访问策略符合预期 |
| `--memory-max-injected-chars` | 小值/大值 | 注入文本长度受限 |
| `--memory-max-source-bytes` | 小值触发截断 | 大源读取受限并有可观察结果 |
| `--memory-strict` | true/false | true 下错误直接失败；false 下降级处理 |
| `--memory-runtime-mode` | `compatibility/scoped/非法值` | compatibility 注入 `memory_diagnostics`；scoped 注入 `memory_retrieval`；非法值报错 |
| `--memory-disable` | 开关 | 关闭后 memory middleware 不注入内容（当前建议补测） |
| `--actor-user-id` | 设置/不设置 | user scope 读写可见性符合 actor 身份 |
| `--actor-thread-id` | thread A/thread B | thread scope 严格线程隔离 |
| `--actor-workspace-id` | 单值/多值 | workspace scope 访问按工作区白名单控制 |

#### 10.3.5 Runtime 保护与缓存参数

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--max-steps` | 足够值/过小值 | 过小触发 `max_steps_exceeded` |
| `--provider-timeout-ms` | 正常值/极小值 | 超时触发 `provider_timeout` |
| `--prompt-cache` | `memory`/其他后端值 | memory 后端触发 provider cache 事件 |
| `--prompt-cache-l2` | 开关 | trace 出现 L2 相关行为或字段 |
| `--prompt-cache-ttl-ms` | 小 TTL | 缓存过期后 miss 行为可观察 |
| `--prompt-cache-max-entries` | 小容量 | 触发淘汰行为可观察 |
| `--summarization-disable` | 开关 | 关闭后不进行摘要中间件处理 |
| `--summarization-max-char-budget` | 小预算 | 历史可见文本被预算约束 |
| `--summarization-max-turns-visible` | 小值 | 可见轮次符合上限 |
| `--summarization-min-recent-messages` | 设置值 | 最近消息保留策略生效 |
| `--summarization-redact-tool-args` | true/false | true 时工具参数脱敏 |
| `--summarization-max-tool-arg-chars` | 小值 | 长参数被截断 |
| `--summarization-truncate-keep-last` | 小值 | 截断后尾部保留条数符合设置 |

#### 10.3.6 中断、事件与输出参数

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--interrupt-on` | 指定单工具/多工具 | 命中指定工具时进入 Interrupted |
| `--events-jsonl` | 指定路径 | 文件存在且每行为合法 JSON 事件 |
| `--stream-events` | 开关 | stderr 输出 provider/runtime 事件 |
| `--pretty` | 开关 | 仅格式化差异，不影响字段语义 |
| `--input` | 普通文本/空串/超长文本 | 输入被正确写入首条 user message |

### 10.4 `skill` 命令族

#### 10.4.1 `skill init`

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `<dir>` | 新目录/已存在目录 | 模板文件创建成功；重复初始化错误稳定 |
| `--pretty` | 开关 | 输出结构不变 |

#### 10.4.2 `skill validate`、`skill list`、`skill install`

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--source` | 缺失、单值、多值 | 缺失时报 `--source is required`；多源聚合正确 |
| `--registry`（install） | 默认/自定义目录 | 安装结果写入指定 registry |
| `--pretty` | 开关 | 输出结构不变 |

#### 10.4.3 `skill status`、`versions`

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--registry` | 默认/自定义 | 读取来源正确 |
| `<name>`（versions） | 存在/不存在 | 存在返回版本列表；不存在返回空或稳定错误 |
| `--pretty` | 开关 | 输出结构不变 |

#### 10.4.4 `skill enable`、`disable`、`quarantine`、`remove`

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `<identity>` | `name`、`name@version`、非法 identity | parse 规则符合约束，非法 identity 返回 `invalid_identity` |
| `--registry` | 默认/自定义 | 生命周期变更落到正确 registry |
| `--reason`（quarantine） | 显式 reason/默认 reason | 未传时默认为 `quarantined_by_cli` |
| `name@version` 强制（quarantine/remove） | 仅 name 输入 | 返回 `invalid_arguments` |
| 治理阻断（enable） | fail 状态技能 enable | 返回 `governance_blocked` |

#### 10.4.5 `skill resolve`

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--input` | 触发词命中/未命中 | selected/skipped/candidates 数量合理 |
| `--registry`、`--source` | registry + source 叠加 | snapshot 与 diagnostics 合并正确 |
| `--skill`、`--disable-skill` | 显式启停 | selected 与 skipped 理由可解释 |
| `--skill-select` | `auto/manual/off/非法` | 语义正确；非法值报错 |
| `--skill-max-active` | `0/1/N` | 实际上限最小为 1 |
| `--refresh-skill-snapshot` | 开/关 | refresh 行为可观察 |
| `--pretty` | 开关 | 输出结构不变 |

#### 10.4.6 `skill audit`

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--thread-id` | 存在/不存在 | 存在返回 record；不存在返回 `audit_not_found` |
| `--root` | 默认/显式 root | audit 路径解析正确 |
| `--pretty` | 开关 | 输出结构不变 |

### 10.5 `memory` 命令族

#### 10.5.1 `memory put` 与 `memory remember`

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--key` | 普通键/重复键 | 重复写入更新行为符合预期 |
| `--value` | 普通文本/空串/大文本 | 写入成功且可读取 |
| `--title` | 传/不传 | 元数据保存正确 |
| `--scope` | `user/thread/workspace` | 作用域写入正确 |
| `--scope-id` | 显式传入/由 actor 推导 | scope_id 解析与持久化正确 |
| `--type` | `semantic/procedural/episodic/pinned` | memory_type 保存正确 |
| `--pinned`（put） | 开/关 | pinned 标记生效 |
| `--tag` | 0/1/多标签 | 查询按 tag 命中 |
| `--actor-user-id` | 传/不传 | user scope 写权限校验正确 |
| `--actor-thread-id` | 传/不传 | thread scope 写权限校验正确 |
| `--actor-workspace-id` | 单值/多值 | workspace scope 写权限校验正确 |
| `--store` | 默认/自定义路径 | 文件落盘路径正确 |
| `--pretty` | 开关 | 输出结构不变 |
| `remember` 默认行为 | 无 `--scope/--type` | 默认 scope=user 且 pinned=true，符合“长期记忆”语义 |

#### 10.5.2 `memory get` 与 `memory explain`

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--key` | 存在/不存在/已删除 | get 的可见性与 explain 的诊断字段一致 |
| `--actor-user-id` | 不同用户读取同 key | 跨用户不可见 |
| `--actor-thread-id` | 不同线程读取同 key | thread scope 隔离生效 |
| `--actor-workspace-id` | 不同工作区读取同 key | workspace scope 隔离生效 |
| `--store` | 默认/自定义 | 数据源路径正确 |
| `--pretty` | 开关 | 输出结构不变 |

#### 10.5.3 `memory edit`、`pin`、`unpin`、`delete`

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `edit --value/--title` | 更新部分字段 | `updated=true` 且字段变更正确 |
| `edit --scope/--scope-id` | 跨 scope 迁移 | 迁移后读写权限与可见性符合新 scope |
| `edit --type` | 类型切换 | memory_type 更新正确 |
| `edit --confidence/--salience` | 合法值/越界值 | 合法值生效；越界报 `invalid_arguments` |
| `edit --clear-tags` + `--tag` | 清空后重建标签 | 标签结果与输入一致 |
| `pin/unpin --key` | 激活项/已删除项 | 激活项可更新；已删除项不可更新 |
| `delete --key` | 命中/未命中 | 命中 `deleted=true` 且 status=deleted；未命中 `deleted=false` |
| actor 参数 | 非授权 actor 执行变更 | 写入被拒绝 |

#### 10.5.4 `memory query` 与 `memory compact`

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--prefix`（query） | 匹配/不匹配 | entries 过滤正确 |
| `--tag`（query） | 匹配/不匹配 | tag 过滤正确 |
| `--scope` / `--scope-id`（query） | 按作用域过滤 | 只返回目标 scope |
| `--type`（query） | 类型过滤 | memory_type 过滤正确 |
| `--pinned`（query） | true/false | pinned 过滤正确 |
| `--status`（query） | active/deleted | 状态过滤正确 |
| `--include-inactive`（query） | 开/关 | deleted/inactive 是否返回符合预期 |
| actor 参数（query） | 跨作用域查询 | 仅返回 actor 可读记录 |
| `--limit`（query） | 默认 `50`、`1`、大值 | 返回数量受限且稳定 |
| `--store` | 默认/自定义 | 数据源路径正确 |
| `--pretty` | 开关 | 输出结构不变 |
| `compact`（策略） | `lru/fifo/ttl` 配置下执行 | evicted 数与剩余条目符合策略预期 |

#### 10.5.5 memory E2E 覆盖状态（建议同步）

- 已覆盖：scoped 注入、strict 降级、生命周期命令、actor 访问控制、LRU/FIFO/TTL 策略。
- 建议补测：`run --memory-disable`（当前未见稳定 E2E 用例）。

### 10.6 `config` 命令族

#### 10.6.1 `config list/get`

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--scope` | `effective/workspace/global` + 非法值 | 合法 scope 可解析；非法 scope 报错 |
| `key`（get） | 存在/不存在/非法 key | 存在返回值；不存在返回 null；非法 key 报错 |
| `--pretty` | 开关 | 输出结构不变 |

#### 10.6.2 `config set/unset`

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `key` | 合法键/非法键 | 合法键写入成功；非法键 parse 失败 |
| `value`（set） | bool/int/string/path/secret-like | 类型转换符合 schema，secret 不明文泄漏 |
| `--scope` | 默认 workspace、显式 global/workspace | 生效层级正确，后续 get 可验证 |
| `--pretty` | 开关 | 输出结构不变 |

#### 10.6.3 `config schema/doctor`

| 参数 | 关键场景 | 断言要点 |
|---|---|---|
| `--pretty` | 开关 | 输出结构不变，schema/doctor JSON 可解析 |

## 11. 执行建议：参数覆盖优先级

为降低一次性工作量，建议按优先级补齐参数级 E2E：
- P0：会改变安全边界或退出码的参数（`--provider`、`--mock-script`、`--tool-choice`、`--skill-select`、`--max-steps`、`--provider-timeout-ms`、`--interrupt-on`、`--scope`）。
- P1：会改变状态持久化或可观测性的参数（`--state-file`、`--thread-id`、`--events-jsonl`、`--audit-json`、`--refresh-skill-snapshot`）。
- P2：会改变质量属性但不直接改变主流程结果的参数（`--pretty`、summarization/prompt-cache 细项）。

建议每个参数至少落地 2 条用例：
- 1 条成功路径（参数生效）
- 1 条失败或边界路径（非法值、缺失依赖、最小/最大值）
