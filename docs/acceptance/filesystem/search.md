---
title: Core Filesystem E2E - 搜索（glob/grep）
scope: core
---

## 1. 端到端效果

搜索能力必须保证：

- glob/grep 的输出格式稳定，可被 CLI/UI/测试解析
- grep 支持 output_mode：files_with_matches/content/count
- 输出不泄露真实磁盘路径
- 对大输出具备明确的截断或 offload 策略（grep 通常会被截断，超大则走 offload 或 tool 自身截断，取决于实现，但必须固定）

## 2. 验收环境

- backend=FilesystemBackend(tempdir_workspace)
- workspace 内放置：
  - `/a.txt`：包含多行 "hello"
  - `/b.txt`：不包含 "hello"
  - `/dir/c.txt`：包含 "hello"

## 3. E2E 场景（必测）

### FSE-01：glob 默认 path="/"，支持递归匹配

给定：

- pattern="**/*.txt"

当：glob(pattern="**/*.txt")（不传 path）

则：

- 返回包含 `/a.txt` `/b.txt` `/dir/c.txt`
- 结果顺序固定（建议字典序）

### FSE-02：glob 指定 path 子树

当：glob(pattern="**/*.txt", path="/dir")

则：

- 返回仅包含 `/dir/c.txt`

### FSE-03：grep output_mode=files_with_matches

当：grep(pattern="hello", output_mode="files_with_matches")

则：

- 返回包含 `/a.txt` 与 `/dir/c.txt`
- 不包含 `/b.txt`

### FSE-04：grep output_mode=count

当：grep(pattern="hello", output_mode="count")

则：

- 输出包含每个文件的匹配数量
- `/b.txt` 的数量为 0 或不出现（二选一，但必须固定）

### FSE-05：grep output_mode=content（包含行号）

当：grep(pattern="hello", output_mode="content")

则：

- 每条匹配输出至少包含：path、line、text
- line 是 1-based（建议对齐 Python 的 GrepMatch.line）

### FSE-06：grep glob 过滤参数（只查某些文件）

当：grep(pattern="hello", glob="a.*", output_mode="files_with_matches")

则：

- 只返回 `/a.txt`

### FSE-07：grep 的 path 参数作用域

当：grep(pattern="hello", path="/dir", output_mode="files_with_matches")

则：

- 只返回 `/dir/c.txt`

### FSE-08：输出格式稳定性（golden）

给定：

- 固定文件内容与固定排序

当：连续两次执行同一 grep/glob

则：

- 输出字符串（或结构化 JSON）一致，可用于 golden snapshot

## 4. 通过标准

- FSE-01 ~ FSE-08 全通过

