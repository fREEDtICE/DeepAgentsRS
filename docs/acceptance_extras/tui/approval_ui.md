---
title: Extras TUI E2E - 审批流（HITL UI）
scope: extras
---

## 1. 端到端效果

当 Core 触发 Interrupt（HITL）时，TUI 必须提供可操作的审批界面，端到端效果包括：

- 明确展示待审批的工具调用（tool_name、path、args 摘要）
- 提供 approve/reject/edit 三类操作（如果 Core 支持）
- 用户选择后，TUI 能将 resume 载荷送回 runner 并继续执行
- 全流程不丢 tool_call_id，对齐可诊断

## 2. 验收方法

- ScriptedModel 驱动产生 interrupt_on 的工具调用
- TUI 测试框架以“键盘脚本”驱动操作（例如按键序列）
- 断言：
  - 屏幕快照（审批弹窗/侧栏）
  - 工具副作用（文件是否写入）
  - events.jsonl 中的 resume 决策记录（推荐）

## 3. E2E 场景（必测）

### TA-01：approve 路径

给定：

- interrupt_on={"write_file":true}
- tool_call：write_file("/a.txt","1")

当：

- TUI 弹出审批界面
- 用户按键选择 approve

则：

- a.txt 被写入
- UI 显示“已批准并执行”

### TA-02：reject 路径

给定：

- interrupt_on={"write_file":true}
- tool_call：write_file("/deny.txt","x")

当：用户选择 reject

则：

- deny.txt 不存在
- UI 显示“已拒绝/已取消”

### TA-03：edit 路径（修改参数）

给定：

- interrupt_on={"write_file":true}
- tool_call：write_file("/a.txt","1")

当：

- 用户进入编辑界面，把 file_path 改为 "/b.txt"，content 改为 "2"
- 提交并 approve

则：

- b.txt 写入成功
- a.txt 不存在

### TA-04：多次 interrupt 的连续审批

给定：

- 同一轮 tool_calls：[write_file(id=a,...), edit_file(id=b,...)]

当：

- 先对 a approve
- 再对 b approve

则：

- UI 不出现错位审批（a/b 不混淆）
- 执行顺序正确

## 4. 通过标准

- TA-01 ~ TA-04 全通过

