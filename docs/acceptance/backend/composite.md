---
title: Core Backend E2E - CompositeBackend 路由与隔离
scope: core
---

## 1. 端到端效果（路由）

CompositeBackend 的作用是把不同虚拟路径前缀路由到不同 backend，从而把 offload/历史等副作用隔离到专用目录。端到端效果需要满足：

- 路由是“最长前缀匹配”
- 路由后传给子 backend 的路径剥离前缀，并保持以 `/` 开头
- 对“无尾斜杠的根路径”存在兼容规则（对齐 Python）

参考 Python： [composite.py](../../../../deepagents/libs/deepagents/deepagents/backends/composite.py)。

## 2. 验收环境

- default = FilesystemBackend(tempdir_workspace)
- routes：
  - `/conversation_history/` → FilesystemBackend(tempdir_history)
  - `/large_tool_results/` → FilesystemBackend(tempdir_large)
  - `/memories/` → StateBackend（用于边界条件）

## 3. E2E 场景（必测）

### BC-01：最长前缀匹配

给定：

- routes 同时包含：
  - `/large/` → A
  - `/large/tool_results/` → B

当：写入路径 `/large/tool_results/x`

则：

- 必须路由到 B（最长前缀）

### BC-02：前缀剥离与 `/` 保持

给定：

- route_prefix=`/conversation_history/` 指向 history backend

当：写入 `/conversation_history/e2e_thread.md`

则：

- 子 backend 接收到的路径为 `/e2e_thread.md`
- 真实落盘在 tempdir_history/e2e_thread.md（由 FilesystemBackend 决定）

### BC-03：根路径的无尾斜杠兼容

给定：

- route_prefix=`/memories/` 指向某 backend

当：

- 访问 `/memories`
- 访问 `/memories/`

则：

- 两者都路由到 memories backend
- 子 backend 接收到的路径都为 `/`

### BC-04：未命中路由走 default

给定：

- routes 不包含 `/work/`

当：写入 `/work/a.txt`

则：

- 路由到 default backend

### BC-05：offload 不污染 workspace

给定：

- 某能力触发写入：
  - `/conversation_history/e2e_thread.md`
  - `/large_tool_results/xyz`

当：执行写入

则：

- 两个文件分别只出现在 tempdir_history 与 tempdir_large
- tempdir_workspace 下不出现 conversation_history 或 large_tool_results 目录

### BC-06：路由后的路径校验交互

给定：

- 针对 `/conversation_history/../x` 的写入尝试

当：通过工具层触发写入

则：

- 若工具层做 validate_path，应在到达 backend 前拒绝（推荐）
- 不允许靠路由剥离产生路径穿越

## 4. 通过标准

- BC-01 ~ BC-06 全通过
- 所有断言只使用虚拟路径与受控临时目录，不依赖真实环境路径

