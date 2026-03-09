---
title: Core Filesystem E2E - 文件操作（ls/read/write/edit/图片）
scope: core
---

## 1. 端到端效果

文件操作能力必须保证：

- 所有路径是虚拟绝对路径（以 `/` 开头）
- `read_file` 按行分页（offset/limit），默认 offset=0 limit=100
- `write_file` 与 `edit_file` 的副作用在同一 backend 中可被后续 read/ls 观察到
- `read_file` 对图片扩展名走“多模态返回”（而不是文本）
- read_file 自身的“长内容截断”不应触发 large tool result offload（Python 侧排除 read_file）

## 2. 验收环境

- backend=FilesystemBackend(tempdir_workspace)，路径均映射到受控临时目录
- 固定 thread_id（便于与 offload/summarization 组合验收）

## 3. E2E 场景（必测）

### FOP-01：ls 返回虚拟路径列表

给定：

- workspace 里有 `/dir/a.txt` `/dir/b.txt`

当：ls("/dir")

则：

- 返回包含 `/dir/a.txt` 与 `/dir/b.txt`
- 不得包含 tempdir 的真实路径字符串

### FOP-02：read_file 默认分页参数

给定：

- `/a.txt` 有超过 100 行

当：read_file("/a.txt")（不传 offset/limit）

则：

- 返回恰好 100 行（或按实现的截断提示策略，但必须可判定为默认 100）
- 提示如何继续读取（offset/limit）

### FOP-03：read_file 的 offset/limit 按行语义

给定：

- `/a.txt` 内容为 5 行：L1..L5

当：

- read_file("/a.txt", offset=0, limit=2)
- read_file("/a.txt", offset=2, limit=2)
- read_file("/a.txt", offset=4, limit=2)

则：

- 返回分别包含 L1-L2、L3-L4、L5
- offset 为 0-based（与 Python 一致）

### FOP-04：write_file 写入后可读可见

给定：

- write_file("/dir/a.txt","x\ny\n")

当：

- ls("/dir")
- read_file("/dir/a.txt",0,10)

则：

- ls 可见 `/dir/a.txt`
- read 能读到 "x" 与 "y"

### FOP-05：edit_file 单次替换（replace_all=false）

给定：

- `/a.txt` 内容为 `foo foo foo`

当：

- edit_file("/a.txt", old_string="foo", new_string="bar", replace_all=false)
- read_file("/a.txt",0,10)

则：

- 内容为 `bar foo foo`

### FOP-06：edit_file 全量替换（replace_all=true）

给定：

- `/a.txt` 内容为 `foo foo foo`

当：

- edit_file("/a.txt", old_string="foo", new_string="bar", replace_all=true)

则：

- 内容为 `bar bar bar`

### FOP-07：edit_file 未匹配的错误语义

给定：

- `/a.txt` 内容不包含 old_string

当：edit_file("/a.txt", old_string="ZZZ", new_string="bar", replace_all=false)

则（二选一，必须固定）：

- 方案 A：返回明确错误（未找到 old_string），文件不变
- 方案 B：返回成功但 occurrences==0，文件不变

### FOP-08：read_file 图片分支（多模态）

给定：

- `/img.png` 是有效 PNG

当：read_file("/img.png")

则：

- 返回多模态 ToolMessage（包含 image/png + 数据）
- 元信息包含虚拟路径 `/img.png`（字段名按实现固定）

### FOP-09：read_file 长内容截断不应写入 /large_tool_results

给定：

- `/big.txt` 极长，read_file 本身会截断

当：read_file("/big.txt",0,100)

则：

- 不会出现对 `/large_tool_results/...` 的写入副作用
- 返回提示指导继续分页读取

## 4. 通过标准

- FOP-01 ~ FOP-09 全通过
- 事件流能断言 tool_call_id 对齐与回注顺序

