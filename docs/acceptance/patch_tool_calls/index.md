---
title: PatchToolCalls 验收索引（Core）
scope: core
---

PatchToolCalls 目标：修复历史 messages 中“悬挂 tool_call”（assistant 发起 tool_call 但缺少对应 ToolMessage），保证继续运行前历史一致。

详细 E2E 见总览文档：

- [patch_tool_calls.md](../patch_tool_calls.md)

本索引页用于在 ACCEPTANCE_CORE.md 中提供统一入口；后续如需进一步拆分（检测算法/补齐策略/边界案例），可在此目录追加子文档。

