---
title: Core 验收 - Filesystem（工具语义 / 安全 / 大结果落盘）
scope: core
---

## 1. 能力定义（E2E 效果）

Filesystem 能力的端到端效果是：Agent 能在受控虚拟文件系统中进行查询与修改，并且：

- 工具 schema、默认参数与返回形态稳定
- 所有路径遵循“虚拟绝对路径（以 / 开头）”规则
- 安全边界可恢复：非法路径必须被拒绝且不会产生副作用
- 大结果不会把上下文打爆：超过阈值会落盘到 `/large_tool_results/...` 并返回引用
- execute 必须由 sandbox backend 能力决定是否暴露

参考 Python 实现：

- FilesystemMiddleware： [filesystem.py](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py)
- validate_path： [utils.py:L234-L297](../../../deepagents/libs/deepagents/deepagents/backends/utils.py#L234-L297)

## 2. 工具契约（Schema 与默认值）

验收时必须对齐以下“对外契约”（以 Python 为准）：

- `ls(path: str) -> str`
- `read_file(file_path: str, offset: int=0, limit: int=100) -> str | ToolMessage(多模态)`
- `write_file(file_path: str, content: str) -> str | Command(update=...)`
- `edit_file(file_path: str, old_string: str, new_string: str, replace_all: bool=false) -> str | Command(update=...)`
- `glob(pattern: str, path: str="/") -> str`
- `grep(pattern: str, path: str|None=None, glob: str|None=None, output_mode: "files_with_matches"|"content"|"count"="files_with_matches") -> str`
- `execute(command: str, timeout: int|None=None) -> str`（仅在 sandbox backend 可用）

除此之外，验收还必须覆盖：

- `read_file` 的分页语义（offset/limit 按“行”）
- `read_file` 图片分支（png/jpg/jpeg/gif/webp）：返回多模态 ToolMessage，并携带 `read_file_path/read_file_media_type` 等元信息
- `execute.timeout` 的上下界校验（>=0 且 <= max_execute_timeout）

## 3. 验收环境

推荐组合（保证可复现且可断言）：

- `CompositeBackend`
  - default = FilesystemBackend(tempdir_workspace)
  - `/large_tool_results/` → FilesystemBackend(tempdir_large)
- 固定 thread_id="e2e_thread"
- 将 large offload 阈值调小（例如 50 tokens）以稳定触发

## 4. E2E 场景（Filesystem 必测）

### FS-01：ls 基本语义（只返回虚拟路径列表）

给定：

- workspace 下存在 `/dir/a.txt` 与 `/dir/b.txt`
- 模型发起 tool_call：ls(path="/dir")

当：执行工具

则：

- ToolMessage 返回只包含 path 列表（不泄露真实磁盘路径）
- 返回结果可被稳定解析（例如每行一个 path 或 JSON 数组，二者择一但需固定）
- 结果长度超过上限时发生截断，并出现截断提示（若实现了截断）

### FS-02：read_file 分页（offset/limit 按行）

给定：

- `/a.txt` 内容为 5 行

当：

- read_file("/a.txt", offset=0, limit=2)
- read_file("/a.txt", offset=2, limit=2)

则：

- 第一次返回第 1-2 行，第二次返回第 3-4 行
- offset 为 0-based（与 Python 工具一致）
- 任何一轮返回都不会包含超出 limit 的额外行

### FS-03：read_file 超长截断（不走 large offload）

给定：

- `/big.txt` 单文件极长，read_file 本身触发内部截断阈值

当：read_file("/big.txt", offset=0, limit=100)

则：

- 返回文本被截断并在末尾包含“已截断/用 offset+limit 继续读取”的提示
- 不应写入 `/large_tool_results/...`（Python 明确把 read_file 排除在 offload 之外）

### FS-04：read_file 图片分支（多模态结果）

给定：

- `/img.png` 是有效图片文件

当：read_file("/img.png", offset=0, limit=100)

则：

- tool 返回类型为多模态 ToolMessage（而非纯字符串）
- 结果中包含 media_type（如 image/png）与数据（bytes 或 base64，取决于 Rust 表达）
- 元信息包含 `read_file_path="/img.png"` 与 `read_file_media_type="image/png"`（字段名对齐 Python 即可）

### FS-05：write_file 写入与可见性

给定：

- 模型调用 write_file(file_path="/dir/a.txt", content="x\ny\n")

当：执行工具

则：

- workspace 落盘成功（tempdir_workspace/dir/a.txt）
- 后续 ls("/dir") 可见该文件
- 后续 read_file("/dir/a.txt",0,10) 可读到完整内容

### FS-06：edit_file 单次替换（replace_all=false）

给定：

- `/a.txt` 内容：`foo foo foo`

当：

- edit_file("/a.txt", old_string="foo", new_string="bar", replace_all=false)

则：

- 文件内容变为 `bar foo foo`（只替换第一次出现）
- 返回结果包含 occurrences（如果 Rust 选择对齐该字段），或至少能从消息中判定替换次数

### FS-07：edit_file 全量替换（replace_all=true）

给定：

- `/a.txt` 内容：`foo foo foo`

当：

- edit_file("/a.txt", old_string="foo", new_string="bar", replace_all=true)

则：

- 文件内容变为 `bar bar bar`

### FS-08：glob 的 path 默认值与超时语义

给定：

- workspace 有多层目录结构

当：

- glob(pattern="**/*.txt", path="/")

则：

- 返回所有匹配 `.txt` 的虚拟路径列表
- 若 glob 操作耗时过长，必须在超时后以明确错误返回（而不是卡死）

### FS-09：grep output_mode 的三种形态

给定：

- `/a.txt` 与 `/b.txt` 含匹配与不匹配行

当：

- grep(pattern="hello", output_mode="files_with_matches")
- grep(pattern="hello", output_mode="count")
- grep(pattern="hello", output_mode="content")

则：

- files_with_matches：只列文件路径
- count：列出每个文件匹配数量（格式固定）
- content：列出匹配行（至少包含 path + line + text）

判定重点：输出格式必须稳定，便于 CLI/UI 或测试解析。

### FS-10：validate_path 拒绝穿越与家目录

给定：

- 工具调用 read_file("/../x")
- 工具调用 write_file("~/x","1")
- 工具调用 write_file("C:\\x","1")

当：执行工具

则：

- 全部返回明确错误
- workspace 与 offload 目录都不产生任何新文件
- runner 可继续执行后续轮次（错误可恢复）

### FS-11：execute 仅在 sandbox backend 可用

给定：

- Case A：default backend 不是 sandbox
- Case B：default backend 是 sandbox（能执行 `echo 1`）

当：

- Case A：模型尝试调用 execute
- Case B：模型调用 execute("echo 1", timeout=10)

则：

- Case A：execute 不出现在 tools 中，或调用返回明确错误（必须固定）
- Case B：返回包含输出 "1" 且 exit_code==0（若提供）

### FS-12：execute timeout 上下界

给定：

- sandbox backend

当：

- execute("sleep 2", timeout=0)（允许 0 代表“不等待/立即超时”或“禁用超时”，但语义必须固定且文档化）
- execute("echo 1", timeout=-1)
- execute("echo 1", timeout=max_execute_timeout+1)

则：

- timeout=-1 与超上限必须返回明确参数错误
- 其余情况按定义行为执行，并可从结果判定

### FS-13：large tool result offload（非 read_file 系列）

给定：

- 准备一个会产生超大输出的工具结果（推荐用 grep(content) 在一个巨大文件上匹配，或用专门的 test tool）

当：触发工具返回超大字符串

则：

- `/large_tool_results/{tool_call_id}` 写入完整内容
- ToolMessage 被替换为引用模板（包含写入虚拟路径与 head/tail 预览）
- 后续通过 read_file("/large_tool_results/{id}", offset, limit) 能分页读取该落盘内容

## 5. 通过标准

- FS-01 ~ FS-13 全通过
- artifacts 中必须包含：
  - workspace 目录快照（用于写/改/读/ls/glob/grep 断言）
  - large 目录快照（用于 offload 断言）
  - events.jsonl 与 final_state.json（用于 tool_call_id 与回注顺序断言）

