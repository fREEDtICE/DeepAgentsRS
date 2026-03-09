---
title: Core 验收 - Backend（协议 / 路由 / 虚拟路径）
scope: core
---

## 1. 能力定义（E2E 效果）

Backend 是 Core 的“环境抽象”。端到端效果需要满足：

- Runner 与 middleware 只依赖 backend 协议，不直接依赖本地磁盘或 shell
- 同一套工具在不同 backend 上表现一致（语义一致，副作用落点不同）
- CompositeBackend 能把“虚拟路径前缀”路由到不同 backend，使：
  - `/conversation_history/...` 与 `/large_tool_results/...` 可写入独立落盘位置
  - 工作区文件不被 offload 污染

## 2. 协议与可观察语义（对齐 Python）

对齐 Python 的关键结构（源： [protocol.py](../../../deepagents/libs/deepagents/deepagents/backends/protocol.py)）：

- `ls_info` 返回 `FileInfo{path,is_dir?,size?,modified_at?}`
- `read(path, offset, limit)` 按行分页（offset/limit 语义与 tools 文档对齐）
- `glob_info(pattern, path)` 返回匹配文件的 `FileInfo.path`
- `grep_raw(pattern, path, glob, output_mode)` 返回结构化 `GrepMatch{path,line,text}` 或明确错误
- `write/edit` 返回 `WriteResult/EditResult`，允许携带 `files_update`（用于 StateBackend 的“写入反映到 state”语义）
- `execute` 只在 SandboxBackend 存在（能力 gating）

## 3. 验收环境

必须至少提供三种 backend 组合用于 E2E：

- `StateBackend`：所有文件都存在于 state（用于验证 `files_update`/state patch 的合并语义）
- `FilesystemBackend(root_dir=tempdir)`：受控真实目录（用于验证真实落盘内容）
- `CompositeBackend{routes, default}`：
  - default = FilesystemBackend(tempdir_workspace)
  - `/conversation_history/` → FilesystemBackend(tempdir_history)
  - `/large_tool_results/` → FilesystemBackend(tempdir_large)

## 4. E2E 场景（Backend 必测）

### B-01：StateBackend 写入反映为 state patch（files_update）

给定：

- backend=StateBackend
- 通过 filesystem 工具调用 write_file("/a.txt","x")

当：Runner 执行该工具并合并结果

则：

- final_state 中存在可定位的文件集合（例如 `state.files["/a.txt"]=="x"`，具体 key 由 Rust 设计决定，但必须一致且可断言）
- ToolMessage 明确表示写入成功，并包含路径
- 同一路径再次 read_file 能读到最新内容

判定重点：StateBackend 不是“在磁盘写文件”，而是“通过 Command.update 合并进 state”。

### B-02：FilesystemBackend root_dir 约束（虚拟根）

给定：

- backend=FilesystemBackend(root_dir=tempdir_workspace)
- write_file("/dir/a.txt","x")

当：执行工具

则：

- 实际落盘在 `tempdir_workspace/dir/a.txt`
- 通过 ls("/dir") 能看到 "/dir/a.txt"（虚拟路径）
- 不允许写出 root_dir 之外

判定重点：虚拟路径与真实路径映射一致。

### B-03：CompositeBackend 路由（conversation_history）

给定：

- backend=CompositeBackend（如上配置）
- 任意能力触发写入 "/conversation_history/e2e_thread.md"

当：执行写入

则：

- 文件出现在 tempdir_history 对应位置
- tempdir_workspace 不出现 conversation_history 目录

判定重点：offload 的副作用不污染工作区。

### B-04：CompositeBackend 路由（large_tool_results）

给定：

- backend=CompositeBackend（如上配置）
- 触发大结果 offload 写入 "/large_tool_results/{id}"

当：执行 offload

则：

- 文件出现在 tempdir_large
- ToolMessage 返回引用该虚拟路径（而不是实际磁盘路径）

判定重点：工具层只认识虚拟路径，backend 决定实际落盘。

### B-05：路由边界条件（无尾斜杠与根映射）

给定：

- CompositeBackend 存在 route_prefix="/memories/"（示例）
- 对路径 "/memories" 与 "/memories/" 与 "/memories/x.txt" 分别执行 ls/read/write

当：执行工具

则：

- "/memories" 与 "/memories/" 都能路由到 memories backend 的根（传入子 backend 的路径为 "/"）
- "/memories/x.txt" 传入子 backend 的路径为 "/x.txt"

判定重点：对齐 Python 的“无尾斜杠仍视为根”的语义（源： [composite.py](../../../deepagents/libs/deepagents/deepagents/backends/composite.py)）。

### B-06：execute 能力 gating（协议层）

给定：

- default backend 不支持 execute

当：

- filesystem middleware 组装 tools

则：

- 模型请求中不包含 execute tool schema（推荐），或 execute 调用必返回明确错误

判定重点：能力必须由协议决定，而不是由 prompt 或上层“约定”。

## 5. 通过标准

- B-01 ~ B-06 全通过
- 每个场景都能在 artifacts 中定位到：
  - final_state.json（用于 StateBackend 断言）
  - tempdir_workspace/tempdir_history/tempdir_large 的目录快照（用于落盘断言）

