---
title: Extras 技能生态 E2E - 执行与隔离（权限/沙箱/资源）
scope: extras
---

## 1. 端到端效果

技能执行属于“外部扩展”，必须可控且可隔离。端到端验收关注：

- 权限：技能能否使用 filesystem/execute/network 等能力必须可配置
- 隔离：技能运行错误不影响主 runner；技能不应泄露 secrets
- 资源：技能执行有超时/输出大小/并发上限（避免 DoS）
- 审批：高风险技能调用可接入 HITL（可选但强建议）

## 2. 验收环境

- 提供三类技能：
  - safe-skill：纯计算，无副作用
  - fs-skill：尝试写文件 `/skill.txt`
  - exec-skill：尝试执行 `echo 1`
- 提供一套权限配置（示例）：
  - allow_filesystem: false
  - allow_execute: false
  - allow_network: false

## 3. E2E 场景（必测）

### SEI-01：无权限时禁止 filesystem 副作用

给定：

- allow_filesystem=false
- 模型调用 fs-skill

当：执行技能

则：

- 返回明确错误（权限不足）
- workspace 中不存在 /skill.txt

### SEI-02：无权限时禁止 execute

给定：

- allow_execute=false
- 模型调用 exec-skill

当：执行技能

则：

- 返回明确错误
- 不产生任何命令副作用

### SEI-03：超时与取消

给定：

- long-skill 会 sleep/循环很久
- 配置 skill_timeout=1s

当：调用 long-skill

则：

- 1s 后终止并返回超时错误
- runner 继续下一轮，不崩溃

### SEI-04：大输出处理（截断或 offload）

给定：

- big-skill 返回超大字符串

当：调用 big-skill

则（二选一，必须固定）：

- 方案 A：技能输出走 `/large_tool_results/...` offload
- 方案 B：技能输出截断并提示如何获取完整内容

### SEI-05：技能 panic/异常的传播语义

给定：

- panic-skill 直接 panic

当：调用

则：

- 返回 error ToolMessage（可诊断）
- runner 不崩溃

### SEI-06：HITL 对高风险技能生效（可选但强建议）

给定：

- interrupt_on={"exec-skill":true}

当：调用 exec-skill

则：

- 触发 interrupt，UI/CLI 可 approve/reject/edit

## 4. 通过标准

- SEI-01 ~ SEI-05 必须通过
- SEI-06 若提供 HITL 集成则必须通过

