---
title: Core HITL E2E - resume 载荷协议与错误语义
scope: core
---

## 1. 端到端效果

resume 载荷是上层 UI/CLI 与 Runner 的契约。端到端必须满足：

- 支持 approve/reject/edit 三类决策
- edit 的 args 必须经过工具 schema 校验（缺字段/类型错必须报错）
- resume 无效时不丢失 interrupt，可再次提交

## 2. 建议的 resume 协议（可调整，但必须固定）

- approve：`{"type":"approve"}`
- reject：`{"type":"reject","reason":"...可选..."}`
- edit：`{"type":"edit","args":{...}}`

## 3. 验收环境

- interrupt_on={"write_file":true}
- backend=FilesystemBackend(tempdir_workspace)
- ScriptedModel 第 1 轮输出 write_file(id=a,file_path="/a.txt",content="1")

## 4. E2E 场景（必测）

### HR-01：approve 载荷最小形式

当：

- run → interrupt
- resume={"type":"approve"}

则：

- 文件写入成功
- ToolMessage 对齐 tool_call_id=a

### HR-02：reject 载荷带 reason

当：

- run → interrupt
- resume={"type":"reject","reason":"no"}

则：

- 不写文件
- ToolMessage 内容包含 reason 或可诊断为 reject

### HR-03：edit 载荷必须校验 args 完整性

当：

- run → interrupt
- resume={"type":"edit","args":{"content":"2"}}（缺 file_path）

则：

- 返回明确错误（参数校验失败）
- 不写文件
- interrupt 状态仍然存在（允许再次提交）

### HR-04：edit 载荷校验 args 类型

当：

- resume={"type":"edit","args":{"file_path":123,"content":true}}

则：

- 返回明确错误
- 不写文件

### HR-05：二次提交有效 resume 可继续

当：

- HR-03 失败后再次提交 resume={"type":"edit","args":{"file_path":"/b.txt","content":"2"}}

则：

- b.txt 写入成功
- a.txt 不存在

## 5. 通过标准

- HR-01 ~ HR-05 全通过

