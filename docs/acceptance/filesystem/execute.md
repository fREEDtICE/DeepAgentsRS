---
title: Core Filesystem E2E - execute（暴露规则与 timeout）
scope: core
---

## 1. 端到端效果

execute 的验收关注“工具层语义”，包括：

- execute 是否在 tools 中暴露（由 backend 能力决定）
- 参数校验（timeout 上下界、类型）
- 执行结果形态（output/exit_code/truncated，或等价）
- 不可用时错误语义明确

## 2. 验收环境

- Case A：backend=FilesystemBackend（非 sandbox）
- Case B：backend=SandboxBackend（支持 execute）
- 统一安装 FilesystemMiddleware

## 3. E2E 场景（必测）

### FX-01：非 sandbox 不暴露 execute（推荐）

给定：

- Case A

当：Runner 构造 model tools

则：

- tools 列表中不包含 execute

### FX-02：非 sandbox 下强行调用 execute 的防御

给定：

- Case A
- ScriptedModel 直接产出 tool_call：execute("echo 1")

当：Runner 执行

则：

- 返回明确错误 ToolMessage
- 不产生任何命令副作用

### FX-03：sandbox execute 成功

给定：

- Case B

当：execute("echo 1", timeout=10)

则：

- output 包含 "1"
- exit_code==0（若提供）

### FX-04：timeout 上下界校验

给定：

- Case B
- max_execute_timeout=5

当：

- execute("echo 1", timeout=-1)
- execute("echo 1", timeout=6)

则：

- 都返回参数错误

### FX-05：timeout=0 的语义固定

给定：

- Case B

当：execute("sleep 2", timeout=0)

则（二选一，必须固定并在文档中声明）：

- 方案 A：立即超时
- 方案 B：视为不覆盖 backend 默认 timeout

### FX-06：stderr/非 0 退出码的表达

给定：

- Case B

当：execute("sh -c 'exit 7'", timeout=10)（或等价命令）

则：

- exit_code==7（若提供）
- output/truncated 字段仍可用（或等价）

## 4. 通过标准

- FX-01 ~ FX-06 全通过

