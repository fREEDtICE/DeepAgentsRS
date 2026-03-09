---
title: Backend 验收索引（Core）
scope: core
---

Backend 验收围绕三件事拆分：

- 约束与契约（协议语义、虚拟路径）：本页
- CompositeBackend 路由与隔离： [composite.md](composite.md)
- Sandbox/execute 能力边界： [sandbox.md](sandbox.md)

## 1. 能力定义（E2E 效果）

Backend 是 Core 的环境抽象，必须满足：

- 工具层只处理“虚拟绝对路径”（以 `/` 开头），不暴露真实路径
- 任意 backend 实现都能被 Runner+Middleware 组合使用（通过 trait/协议）
- StateBackend 与 FilesystemBackend 在“可观察语义”上等价（写入后可读、ls 可见），只是副作用落点不同

## 2. 协议契约（必须对齐的语义点）

参考 Python： [protocol.py](../../../../deepagents/libs/deepagents/deepagents/backends/protocol.py)。

端到端验收关注这些可观察点：

- 路径：所有入参都使用虚拟路径（`/a.txt`），输出也必须是虚拟路径
- `ls_info`：返回的 FileInfo.path 必是虚拟路径；目录项顺序需固定（建议字典序）
- `read(offset,limit)`：按行分页，offset/limit 语义固定且可断言
- `write/edit`：要么产生真实落盘（FilesystemBackend），要么通过 Command.update 反映到 state（StateBackend）
- `glob/grep`：输出必须可解析（结构化或稳定文本），且不会泄露真实路径

## 3. E2E 场景（协议层必测）

### BP-01：StateBackend 的写入必须可读可见

给定：

- backend=StateBackend
- write_file("/a.txt","x")

当：后续 read_file("/a.txt",0,10) 与 ls("/")

则：

- read 能读到 "x"
- ls 能列出 "/a.txt"
- 不产生任何真实磁盘文件

### BP-02：FilesystemBackend 的 root_dir 映射一致

给定：

- backend=FilesystemBackend(root_dir=tempdir)
- write_file("/dir/a.txt","x")

当：执行

则：

- 真实落盘在 tempdir/dir/a.txt
- 通过虚拟路径 read/ls 仍可读可见

### BP-03：同一工具序列在两种 backend 上“语义等价”

给定：

- 在 StateBackend 上跑一遍：write→edit→read→grep
- 在 FilesystemBackend 上跑一遍：write→edit→read→grep

当：对比可观察结果

则：

- read 输出一致
- grep 匹配结果一致（路径一致、行号一致）

