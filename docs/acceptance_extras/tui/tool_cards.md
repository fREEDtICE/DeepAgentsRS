---
title: Extras TUI E2E - 工具卡片与差异展示（diff）
scope: extras
---

## 1. 端到端效果

当工具涉及文件读写/编辑时，TUI 应提供“可理解的工具卡片”，至少包含：

- 工具名称、目标路径、关键参数摘要
- 工具结果摘要（成功/失败、影响范围）
- 对 edit_file/write_file 等支持差异展示（diff）：
  - 最少能展示“修改前/修改后”或“统一 diff”之一

## 2. 验收方法

- 用 ScriptedModel 驱动稳定工具调用序列
- 断言方式：
  - 屏幕快照或组件树快照
  - diff 内容可用 golden 文件断言（忽略时间戳等噪声）

## 3. E2E 场景（必测）

### TTC-01：write_file 卡片展示关键字段

给定：

- write_file("/a.txt","x")

当：运行

则：

- UI 卡片包含：tool=write_file、path=/a.txt、status=success
- 工具结果中不直接渲染全部 content（避免污染屏幕），只显示摘要

### TTC-02：edit_file diff 可见且可判定

给定：

- /a.txt="foo"
- edit_file("/a.txt","foo","bar")

当：运行

则：

- 卡片中出现 diff（或 before/after）
- diff 能明确看到 foo→bar

### TTC-03：read_file 长输出的折叠/分页提示

给定：

- read_file 返回很多行（或引用 large_tool_results）

当：运行

则：

- UI 不直接渲染全部文本（允许折叠）
- 有明确提示如何展开/分页查看（例如快捷键或提示文本）

### TTC-04：工具失败时卡片错误态可诊断

给定：

- edit_file old_string 未匹配导致失败

当：运行

则：

- 卡片处于错误态，并显示错误原因

## 4. 通过标准

- TTC-01 ~ TTC-04 全通过

