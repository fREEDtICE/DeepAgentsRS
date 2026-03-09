---
title: Core Subagents E2E - task 工具契约与 subagent 注册
scope: core
---

## 1. 端到端效果

主 agent 通过 `task` 工具启动子 agent。端到端效果需要满足：

- 模型可见的 tool schema 只有：
  - `description: string`
  - `subagent_type: string`
- 系统内部会基于 subagent_type 选择一个已注册的子 agent（spec 或 compiled）
- 子 agent 执行完成后，主线程获得一个 ToolMessage（tool_call_id 对齐）

参考 Python： [subagents.py](../../../../deepagents/libs/deepagents/deepagents/middleware/subagents.py)。

## 2. 注册模型（两类 subagent）

验收要覆盖两类注册形态：

- SpecSubAgent：声明式配置（name/description/system_prompt/model/tools/middleware）
- CompiledSubAgent：直接给 runnable（必须满足输出 state 包含 messages）

## 3. E2E 场景（必测）

### ST-01：subagent_type 未注册的错误语义

给定：

- 主模型调用 task(subagent_type="not-exist")

当：执行 tool

则：

- 返回明确错误 ToolMessage（指出未知 subagent_type，并列出可用类型）
- 不产生任何子线程副作用

### ST-02：SpecSubAgent 的最小成功闭环

给定：

- 注册 spec：name="echo", description="...", tools=[], middleware=[]
- 子模型（脚本）对输入 description 直接输出 Assistant("OK:"+description)
- 主模型调用 task(description="a", subagent_type="echo")

当：运行主 Runner

则：

- 主线程回注 ToolMessage("OK:a")
- tool_call_id 对齐

### ST-03：CompiledSubAgent 的最小成功闭环

给定：

- 注册 compiled runnable：输入 state → 输出 state（包含 messages=[Assistant("C")])
- 主模型调用 task(subagent_type="compiled")

当：执行 tool

则：

- ToolMessage 内容为 "C"

### ST-04：CompiledSubAgent 缺失 messages 的错误语义

给定：

- 注册 compiled runnable：输出 state 不包含 messages
- 主模型调用 task(subagent_type="broken")

当：执行 tool

则：

- 返回明确错误（缺少 messages）
- 主 Runner 不崩溃，可继续下一轮

### ST-05：general-purpose 子 agent 必然存在（默认注入）

给定：

- 不显式注册任何子 agent

当：主模型调用 task(subagent_type="general-purpose")

则：

- 仍可执行（说明系统注入了默认子 agent）
- 子 agent 的 tool 能力至少包含 Filesystem（若 Core 设计如此），或最小可运行闭环

## 4. 通过标准

- ST-01 ~ ST-05 全通过

