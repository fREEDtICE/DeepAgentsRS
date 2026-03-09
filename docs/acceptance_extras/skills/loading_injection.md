---
title: Extras 技能生态 E2E - 加载与注入（system prompt / tool schema）
scope: extras
---

## 1. 端到端效果

技能加载后的关键效果是：模型在每轮调用前都能“看到”新增工具（tools）及其使用说明（system prompt 注入），并且：

- tool schema 正确（参数/必填项/描述）
- tools 列表稳定（顺序与去重规则固定）
- system prompt 中包含技能说明块（可在快照中判定）

## 2. 验收环境

- 使用 ScriptedModel（可断言输入 tools/system）
- skills source 提供至少两个技能：
  - `math-add`：参数 a/b，返回 a+b
  - `echo-skill`：参数 text，返回 "E:"+text
- 通过 CLI 参数或配置启用 skills sources

## 3. E2E 场景（必测）

### SL-01：skills 工具出现在 model tools 中

给定：

- 启用 skills source

当：Runner 构造第 1 轮 ModelRequest

则：

- tools 名称集合包含 math-add 与 echo-skill
- schema 中参数必填项可断言（a/b 必填）

### SL-02：system prompt 注入包含技能说明块

给定：

- 启用 skills source

当：Runner 构造 ModelRequest

则：

- system 中包含技能清单或固定前缀（例如 "## Skills"）
- 能列出每个技能的描述（至少 tool name + 简述）

### SL-03：技能可被模型调用并回注

给定：

- ScriptedModel 输出 tool_call：math-add(a=1,b=2)
- 下一轮输出 Assistant("done")

当：运行

则：

- tool 被执行并返回 "3"（或等价结构）
- ToolMessage 对齐 tool_call_id，并被注入 messages

### SL-04：技能 schema 错误的可诊断失败

给定：

- 一个技能声明 schema 与实现不一致（例如缺少必填参数处理）

当：模型调用该技能

则：

- 返回明确错误（指出参数校验失败或 schema 不匹配）
- 不崩溃，可继续下一轮

### SL-05：skills 与 Core tools 的命名冲突处理

给定：

- 技能 tool 名称与 Core 内置工具冲突（例如 "read_file"）

当：启动 agent

则（二选一，必须固定并文档化）：

- 方案 A：禁止冲突，启动失败并提示冲突
- 方案 B：允许覆盖，但必须显式配置（例如 allow_override=true），且最终生效可诊断

## 4. 通过标准

- SL-01 ~ SL-05 全通过

