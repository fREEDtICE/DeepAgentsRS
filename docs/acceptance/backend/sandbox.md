---
title: Core Backend E2E - Sandbox/execute 能力边界
scope: core
---

## 1. 端到端效果（execute gating）

execute 是高风险能力，必须由 backend 协议能力显式决定：

- 非 sandbox backend：execute 不应暴露给模型（推荐），或调用必返回明确错误（但行为必须固定）
- sandbox backend：execute 可用，且 timeout 语义可判定

参考 Python：execute tool 在 FilesystemMiddleware 中按 backend 能力动态决定是否可用（见 [filesystem.py](../../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py)）。

## 2. 验收环境

- Case A：default backend = FilesystemBackend(tempdir_workspace)（非 sandbox）
- Case B：default backend = SandboxBackend(tempdir_workspace)（支持 execute）
- 统一安装 FilesystemMiddleware（确保 execute 的暴露/过滤由系统决定）

## 3. E2E 场景（必测）

### BS-01：非 sandbox 不暴露 execute（推荐路径）

给定：

- Case A 环境

当：Runner 构造模型请求（tools 列表）

则：

- tools 名称集合中不包含 execute
- ScriptedModel 若断言存在 execute，应当失败（用于强制约束）

### BS-02：非 sandbox 下强行 tool_call execute 的处理（兼容路径）

给定：

- Case A 环境
- ScriptedModel 直接产出 tool_call：execute("echo 1")

当：Runner 执行工具分发

则：

- 返回明确错误 ToolMessage（指出 execute 不可用/后端不支持）
- 不产生任何系统命令副作用

注：如果选择 BS-01 严格不暴露 execute，仍建议实现 BS-02 防御性处理。

### BS-03：sandbox execute 基本成功

给定：

- Case B 环境
- tool_call：execute(command="echo 1", timeout=10)

当：执行

则：

- 返回 output 包含 "1"
- exit_code==0（若实现该字段）

### BS-04：timeout 下界与上界

给定：

- Case B 环境
- max_execute_timeout=5

当：

- execute("echo 1", timeout=-1)
- execute("echo 1", timeout=6)

则：

- 两者都必须返回参数错误（可诊断）

### BS-05：timeout=0 的语义必须固定

给定：

- Case B 环境

当：execute("sleep 2", timeout=0)

则（二选一，必须在实现与文档中固定）：

- 方案 A：立即超时并返回明确超时错误
- 方案 B：视为“禁用覆盖超时”，由 backend 默认超时控制

### BS-06：CompositeBackend 下的 execute 判定以 default 为准

给定：

- CompositeBackend.default=non-sandbox
- routes 中存在一个 sandbox 子 backend（仅用于某些前缀）

当：Runner 构造 tools

则：

- execute 仍不应暴露（对齐 Python：只看 default backend 能力）

## 4. 通过标准

- BS-01 ~ BS-06 全通过
- 所有执行副作用限定在受控 sandbox 内，不访问真实工作区之外

