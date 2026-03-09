---
title: Core Filesystem E2E - 安全边界（validate_path）
scope: core
---

## 1. 端到端效果

Filesystem 的安全边界必须保证：

- 任何工具涉及路径入参时，路径必须经过校验（至少涵盖 ls/read/write/edit/glob；grep 是否校验需明确且固定）
- 校验失败必须可恢复：
  - 返回明确错误（ToolMessage error 或可判定错误文本）
  - 不产生任何副作用（不读、不写、不创建目录）
  - runner 继续运行，不崩溃

参考 Python 的 validate_path 语义： [utils.py:L234-L297](../../../../deepagents/libs/deepagents/deepagents/backends/utils.py#L234-L297)。

## 2. 需要覆盖的禁止模式（对齐 Python）

- 任意 path segment 为 `..`（路径穿越）
- `~` 开头（家目录）
- Windows 盘符绝对路径（例如 `C:\x` 或 `C:/x`）
- 归一化后不以 `/` 开头
- 归一化后仍包含 `..`

## 3. 验收环境

- backend=FilesystemBackend(tempdir_workspace)
- 同时准备一个“诱饵文件”放在 tempdir_workspace 外（例如 tempdir_parent/secret.txt），用于验证无法越界读取

## 4. E2E 场景（必测）

### FSS-01：拒绝 `..` 读

当：read_file("/../secret.txt",0,10)

则：

- 返回 error（明确指向非法路径）
- 不读取 workspace 外内容

### FSS-02：拒绝 `..` 写

当：write_file("/../pwned.txt","x")

则：

- 返回 error
- workspace 外不存在 pwned.txt

### FSS-03：拒绝 `..` 编辑

当：edit_file("/../secret.txt", old_string="a", new_string="b", replace_all=false)

则：

- 返回 error
- workspace 外文件未变化

### FSS-04：拒绝 `~` 前缀

当：write_file("~/x","1")

则：

- 返回 error
- workspace 内外都不出现该文件

### FSS-05：拒绝 Windows 盘符路径

当：

- write_file("C:\\x","1")
- read_file("C:/x",0,10)

则：

- 均返回 error

### FSS-06：拒绝非 `/` 开头

当：read_file("relative/path.txt",0,10)

则：

- 返回 error（要求必须是虚拟绝对路径）

### FSS-07：allowed_prefixes（如果实现该能力）

给定：

- 配置 allowed_prefixes=["/safe/"]

当：

- write_file("/safe/a.txt","1")
- write_file("/unsafe/a.txt","1")

则：

- 前者成功，后者 error

### FSS-08：grep 的 path 校验策略必须固定

当：grep(pattern="x", path="/../", output_mode="files_with_matches")

则（二选一，必须固定并文档化）：

- 方案 A：与其它工具一致，拒绝非法 path
- 方案 B：兼容 Python 现状，不校验，但必须确保 backend 侧不会越界（通常不推荐）

## 5. 通过标准

- FSS-01 ~ FSS-08 全通过
- artifacts 中可证明“无越界副作用”（受控目录外没有新文件、诱饵文件未被读取/修改）

