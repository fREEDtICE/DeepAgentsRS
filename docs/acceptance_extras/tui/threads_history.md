---
title: Extras TUI E2E - 会话/线程切换与历史
scope: extras
---

## 1. 端到端效果

TUI 需要让用户可见地管理线程与历史：

- 创建新线程、切换线程
- 重新打开历史对话（从 checkpoint/store）
- 对每个线程显示基本元信息（thread_id、最近更新时间、标题/摘要可选）

端到端要求：切换线程后 UI 展示的 messages 与操作目标（文件、todo、summarization event）属于对应线程，不串线。

## 2. 验收环境

- 提供可持久化的 checkpointer/store
- ScriptedModel 生成可区分内容：
  - thread t1 输出 "T1"
  - thread t2 输出 "T2"

## 3. E2E 场景（必测）

### TTH-01：创建与切换线程

当：

- 新建线程 t1，发送消息，得到回复 "T1"
- 新建线程 t2，发送消息，得到回复 "T2"
- 切回 t1

则：

- UI 在 t1 中能看到 "T1" 历史
- UI 在 t2 中能看到 "T2" 历史

### TTH-02：线程状态隔离（todo/summarization）

给定：

- t1 触发 write_todos 更新
- t2 不触发

当：在 UI 切换线程查看 todo（如果 UI 提供 todo 面板）

则：

- t1 有 todo，t2 无

### TTH-03：重启 TUI 后恢复线程

给定：

- 运行一次 TUI，产生 t1/t2

当：

- 退出并重启 TUI
- 选择恢复 t1

则：

- 历史可见，且可继续对话

## 4. 通过标准

- TTH-01 ~ TTH-03 全通过

