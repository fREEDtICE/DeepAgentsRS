---
title: Extras TUI E2E - 快照断言机制（屏幕快照 vs 组件树快照）
scope: extras
---

## 1. 为什么需要“快照机制”的验收

TUI 的端到端验收很容易陷入两个极端：

- 只断言 exit code：无法发现 UI 乱序/重复/错位渲染
- 做像素级截图对比：在终端环境不稳定、维护成本高

因此需要把“快照机制”本身作为 Extras 的可交付能力进行验收：它决定了我们后续所有 TUI E2E 用例是否可判定、可维护。

## 2. 两种快照策略（至少支持一种，推荐两种都支持）

### 2.1 屏幕文本快照（Screen Snapshot）

定义：在固定终端尺寸（例如 120x40）下，把当前屏幕可见文本完整 dump 为纯文本文件。

优点：

- 最接近用户实际看到的效果
- 不依赖 UI 组件实现细节

风险：

- 容易受终端宽度、换行规则、ANSI 颜色影响

稳定性要求（必须）：

- 颜色与样式必须可关闭（snapshot 模式强制禁用 ANSI 或统一剥离）
- 宽高固定，并在 artifacts 中记录（例如 metadata.json）
- 时间戳/随机 ID 必须剥离或归一化（见 4.3）

### 2.2 组件树快照（UI Tree Snapshot）

定义：dump 当前 UI 的组件树结构（节点类型、关键字段、层级关系），输出为 JSON/YAML。

优点：

- 不受终端换行影响
- 对 diff 更稳定（字段级别比较）

风险：

- 与 UI 组件实现耦合，需要定义稳定字段集

稳定性要求（必须）：

- 只输出“验收字段集”（例如 widget_type、id、role、text_summary、status）
- 不输出内存地址/对象指针/动态 hash
- 对大文本只输出摘要（len/hash/前缀），避免把敏感或大内容放进快照

## 3. 统一“快照模式”开关（必须）

TUI 必须提供一个明确的快照模式开关（参数名可调整但必须固定），例如：

- `--snapshot-mode`

快照模式下，TUI 必须满足：

- 终端尺寸固定（由框架或参数强制）
- 所有异步动画/光标闪烁/加载 spinner 关闭或冻结
- 所有事件处理具有确定性（避免 timer 驱动的 UI 抖动）

## 4. 端到端快照规范（必须固定）

### 4.1 快照文件布局

建议布局（固定）：

- `ui.snapshots/`
  - `case_<name>/`
    - `step_00_start.screen.txt`（可选）
    - `step_01_after_user_input.screen.txt`
    - `step_02_after_tool_started.tree.json`
    - `step_03_after_tool_finished.screen.txt`
    - `meta.json`（终端尺寸、版本、脱敏策略）

### 4.2 步进时机（Step points）

必须支持在以下关键节点采样（至少覆盖其中 3 个）：

- 用户输入提交后
- 收到 assistant 首个 token 后（流式）
- tool_call 开始（ToolCallStarted）
- tool_call 结束（ToolCallFinished）
- interrupt 弹窗出现
- interrupt 决策提交后

### 4.3 归一化（Normalization）规则

快照必须对下列噪声做归一化，否则无法稳定：

- tool_call_id（可替换为 `<TOOL_CALL_ID>`）
- thread_id（若随机生成，替换为 `<THREAD_ID>`；建议验收固定 thread_id）
- 时间戳（替换为 `<TS>`）
- 进度条/转圈动画（快照模式下应禁用）

并且必须强制脱敏：

- 任何出现在 UI 的 secrets 特征串必须在快照中被替换为 `<REDACTED>`

## 5. E2E 场景（快照机制本身的必测）

### TSN-01：同一输入两次运行快照完全一致

给定：

- snapshot-mode 开启
- ScriptedModel 输出完全确定

当：连续运行两次相同的 TUI 场景

则：

- 对应 step 的 screen snapshot 完全一致（文本一致）
- 对应 step 的 tree snapshot 结构一致（JSON 等价）

### TSN-02：不同终端尺寸下，树快照仍稳定

给定：

- 分别在 120x40 与 100x40 运行（如果支持）

当：采集 tree snapshot

则：

- tree snapshot 结构一致（不受换行影响）

### TSN-03：归一化生效（ID/时间戳不造成 diff）

给定：

- 运行中必然产生 tool_call_id/thread_id

当：采集快照

则：

- 快照中不出现真实 tool_call_id/thread_id/time 字符串
- 只出现归一化占位符

### TSN-04：脱敏生效（UI 不泄露敏感串）

给定：

- 用户输入或文件内容包含 `SECRET_TOKEN_ABC123`

当：采集快照

则：

- 快照中不包含 `SECRET_TOKEN_ABC123`
- 出现 `<REDACTED>`（或固定替换规则）

### TSN-05：快照采样点可控（不会采到中间态抖动）

给定：

- tool 执行会产生“loading”状态

当：

- 在 tool_started 与 tool_finished 两个 step 采样

则：

- tool_started 快照稳定显示 loading
- tool_finished 快照稳定显示结果
- 不存在“同一 step 偶现不同内容”

## 6. 通过标准

- TSN-01/03/04 必须通过
- TSN-02/05 视实现能力纳入，但建议纳入门槛（否则后续 TUI E2E 维护成本会迅速爆炸）

