---
title: Extras TUI 验收索引
scope: extras
---

TUI（交互式终端 UI）的 E2E 验收按“用户可见效果”拆分：

- 基础对话渲染与流式输出： [rendering.md](rendering.md)
- 工具卡片与差异展示（diff）： [tool_cards.md](tool_cards.md)
- 审批流（HITL UI）： [approval_ui.md](approval_ui.md)
- 会话/线程切换与历史： [threads_history.md](threads_history.md)
- 可访问性与键盘操作（基础）： [keyboard_a11y.md](keyboard_a11y.md)
- 快照断言机制（屏幕/组件树）： [snapshots.md](snapshots.md)

说明：TUI 的 E2E 验收不要求像像素级截图一致，但要求“文本屏幕快照/布局树”稳定可断言。
