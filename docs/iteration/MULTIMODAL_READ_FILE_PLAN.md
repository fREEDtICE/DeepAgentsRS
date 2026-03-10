# 多模态 read_file：技术方案与迭代计划

## 结论（对应 ITERATION_PLAN.md L302）

[`ITERATION_PLAN.md#L302`](ITERATION_PLAN.md#L302-L302) 这一条问题是客观存在的：Python（deepagents）在 `read_file` 上能对图片走“二进制下载 + base64 + image content block”的多模态回灌，而 DeepAgentsRS（Rust）当前 `read_file` 仅支持 UTF-8 文本读取，并通过 JSON 输出 `content/truncated/next_offset` 的文本包裹形式返回。

多模态 `read_file` 不是“把图片转成字符串”这么简单，它会牵引三层能力一起收敛：

- 文件后端能力：除了 `read()->String`，还需要受控的二进制读取（或下载）能力
- 工具结果协议：需要能表达 media（类型/尺寸/引用/必要时 base64），并与 Large tool result offload 兼容
- provider/消息承载：需要能把“图片”以模型可理解的消息形态传递（不只是 JSON 字符串里塞 base64）

本方案给出一个与当前 DeepAgentsRS 结构契合、并能逐步演进到“真正多模态喂给模型”的实现路径。

---

## 现状盘点（Rust vs Python）

### Rust（DeepAgentsRS）当前链路

- 工具：`read_file` 固定走后端 `read()`，输出结构为 `{content,truncated,next_offset}`  
  见 [std_tools.rs](../../crates/deepagents/src/tools/std_tools.rs#L66-L123)
- 后端协议：`FilesystemBackend::read(file_path, offset, limit) -> String`，没有 bytes/下载接口  
  见 [backends/protocol.rs](../../crates/deepagents/src/backends/protocol.rs#L17-L53)
- 本地实现：`tokio::fs::read_to_string` + `.lines()`，非 UTF-8（二进制/部分编码文本）会直接失败  
  见 [backends/local.rs](../../crates/deepagents/src/backends/local.rs#L127-L157)
- tool role message：runtime 把 tool result 写成一个 JSON 字符串 envelope（含 `output/content/status/error`），但 `Message.content` 本身只有字符串，没有多模态 blocks  
  见 [simple.rs](../../crates/deepagents/src/runtime/simple.rs#L789-L821)、[types.rs](../../crates/deepagents/src/types.rs#L13-L30)

### Python（deepagents）当前链路

- `read_file` 根据扩展名判断图片：若是图片则走 backend `download_files()` 读取 bytes，再 base64，并返回 `ToolMessage(content_blocks=[image_block])`  
  见 [filesystem.py](../../../deepagents/libs/deepagents/deepagents/middleware/filesystem.py#L559-L667)
- backend 协议区分两条能力：`read()->str` 与 `download_files()->bytes`  
  见 [protocol.py](../../../deepagents/libs/deepagents/deepagents/backends/protocol.py#L197-L227)、[protocol.py](../../../deepagents/libs/deepagents/deepagents/backends/protocol.py#L395-L417)

---

## 目标与非目标

### 目标

- 让 Rust `read_file` 在图片场景下可以返回“可被模型消费的多模态信息”
- 在协议层能表达 media（至少 image），并与 [TOOL_OUTPUT_NORMALIZATION_PLAN.md](TOOL_OUTPUT_NORMALIZATION_PLAN.md) 的 canonical ToolResultEnvelope 收敛方向兼容
- 对大图片/大 base64 有明确的 size 限制与 offload 策略，避免上下文爆炸
- 保持文本读取分页语义不变（offset/limit、cat -n 输出、next_offset）

### 非目标（首期不做）

- 不追求“识别一切二进制类型”（先聚焦 image/png|jpeg|gif|webp）
- 不在首期引入复杂的图片解码/缩放依赖（可作为后续增强）
- 不强制在没有真实 provider（当前主要是 mock）的情况下完成端到端“模型真正看见图片”的集成；但协议与承载必须先铺好

---

## 设计原则（Rust 视角）

- 明确分层：工具产生语义（text/image/offload），renderer 生成稳定 `content`，provider 负责序列化到具体模型协议
- 可控内存：二进制读取必须有上限（max_bytes），避免一次性 `read()` 把大文件拉进内存
- 不泄露敏感信息：错误信息可读但不过度包含绝对路径/系统细节；同时不在日志/trace 中默认打印 base64
- 向后兼容：文本 `read_file` 的输出字段不破坏既有调用方；新能力用新增字段/新分支表达

---

## 总体方案（推荐路径）

把“多模态 read_file”拆为两条并行但强相关的能力闭环：

1) **数据通路（backend/tool）**：增加受控 bytes 读取能力，让 `read_file` 能拿到图片 bytes  
2) **承载通路（message/provider）**：在消息结构中增加可选 `content_blocks`，provider 如支持视觉就发送图片块；否则降级为稳定文本 `content`

这条路径的关键点是：避免把 base64 放进 `output` 并被 offload 逻辑“原样保留”导致上下文爆炸；base64（若必须存在）应进入“多模态块承载”而不是结构化 `output` 的常规字段。

---

## 协议与接口设计

### 1) 工具输入（read_file）

保持现有入参，并新增可选字段（兼容默认行为）：

```json
{
  "file_path": "/abs/path/to/file.png",
  "offset": 0,
  "limit": 100,
  "mode": "auto | text | image",
  "max_bytes": 4000000
}
```

约束：

- `mode=auto`：按扩展名（首期）分流，和 Python 的做法一致
- `max_bytes`：仅对 image/binary 生效；缺省使用运行时默认值（例如 4MB）
- 文本仍走 offset/limit；图片分支忽略 offset/limit（但可以保留字段用于统一 schema）

### 2) 工具输出（read_file output JSON）

建议把 `output` 明确区分为两种变体（tagged union），避免调用方靠字段猜测：

文本：

```json
{
  "type": "text",
  "content": "  1→...\n",
  "truncated": true,
  "next_offset": 100
}
```

图片（首期不在 output 中放 base64，仅放元信息 + 引用）：

```json
{
  "type": "image",
  "file_path": "/abs/path/to/a.png",
  "mime_type": "image/png",
  "size_bytes": 123456,
  "content": "(image returned as content block)"
}
```

### 3) tool message 承载（新增 content_blocks）

在 Rust 的 `Message` 上新增可选字段（不破坏现有字符串 `content`）：

```json
{
  "role": "tool",
  "content": "{\"v\":1,...,\"content\":\"...\"}",
  "content_blocks": [
    { "type": "image_base64", "mime_type": "image/png", "base64": "..." }
  ],
  "tool_call_id": "call_123",
  "name": "read_file",
  "status": "success"
}
```

原则：

- `content` 必须仍然存在，作为稳定可读的降级形态（非视觉模型也能理解）
- `content_blocks` 仅在 provider 支持多模态时才会被编码进真实请求；否则忽略
- 大图片策略：当 `size_bytes > max_bytes` 时，返回结构化错误（`status=error`），并给出可操作提示（例如建议用户缩小图片/只读取 metadata）

> 上述 envelope 的结构建议与 ToolResultEnvelope v1 一致（包含 `v/tool_call_id/tool_name/status/output/error/content/meta`），并把 media 记录进 `meta.media`（参见 [TOOL_OUTPUT_NORMALIZATION_PLAN.md](TOOL_OUTPUT_NORMALIZATION_PLAN.md) 的 `meta.media` 字段建议）。

---

## Rust 侧落点（按模块拆解）

### A) Backend：补齐 bytes 能力

在 [backends/protocol.rs](../../crates/deepagents/src/backends/protocol.rs#L17-L53) 为 `FilesystemBackend` 增加一条“受控二进制读取”能力（二选一）：

- 方案 A（更贴近 Rust）：`read_bytes(file_path, max_bytes) -> Result<Vec<u8>, BackendError>`
- 方案 B（更贴近 Python）：`download_files(paths, max_bytes_each) -> Result<Vec<FileDownloadResponse>, BackendError>`

推荐方案 A：更简单、调用点更少、对 `read_file` 足够；未来若需要批量可再扩展成 B。

LocalSandbox 实现（见 [backends/local.rs](../../crates/deepagents/src/backends/local.rs#L127-L157)）应做到：

- 复用 `resolve_path` 的 root/逃逸约束
- 使用流式读取或 `metadata.len()` 预判以避免一次性加载超大文件
- 明确错误码（例如 `file_not_found/is_directory/too_large/not_utf8`）并映射到稳定的 `error.kind`

### B) Tool：read_file 分支化

在 [std_tools.rs](../../crates/deepagents/src/tools/std_tools.rs#L66-L123)：

- 增加 `mode/max_bytes` 输入字段
- `mode=auto` 时按扩展名判定（与 Python 一致：`.png|.jpg|.jpeg|.gif|.webp`）
- 图片分支：
  - 调用 backend `read_bytes`
  - base64 编码写入 tool message 的 `content_blocks`（不写进 `output`）
  - `output` 仅记录 `{type,image metadata,...}`，并提供稳定 `content` 占位文本

### C) Runtime：ToolResultEnvelope + offload 兼容

当前 offload 逻辑会把 `output` 原样保留，只改/增字段（见 [simple.rs](../../crates/deepagents/src/runtime/simple.rs#L542-L639)）。因此必须避免“把 base64 放进 output”。

落地动作：

- 让 read_file(image) 的 base64 只进入 `Message.content_blocks`
- 把 “media 元信息”写入 envelope `meta.media`（无 base64）
- 进一步的增强（后续）：让 offload 支持“对 content_blocks 的大小评估/裁剪”，并在 `meta.media` 提供 `offload_ref`（例如写到 `/large_tool_results/...`）

### D) Provider：多模态序列化（存在真实 provider 后接入）

provider 接口当前只拿到 `Vec<Message>`（见 [provider/protocol.rs](../../crates/deepagents/src/provider/protocol.rs#L54-L65)），因此最稳妥的做法是把图片数据放在 `Message` 里而不是让 provider 自己读文件系统。

要求：

- provider 在构造请求时：若看到 tool message 的 `content_blocks.image_base64`，且目标模型支持 vision，则按该 provider 的多模态格式发送
- 若不支持 vision：忽略 blocks，只用 `content` 文本

---

## 验收标准（可测试、可复现）

- 文本文件：`read_file` 输出字段与现状一致（`content/truncated/next_offset`），分页行为不变
- 图片文件：`read_file` 输出 `type=image`，tool message 同时包含稳定文本 `content` 与 1 个 image block（base64+mime）
- 大图片：超过 `max_bytes` 时返回结构化错误，且不会在任何 message 中包含部分 base64 片段
- offload 互操作：启用 large tool result offload 时，图片分支不会触发“把 base64 写进 output 并被保留”的路径；`content` 仍可被 offload/裁剪而不丢失 blocks（或明确地拒绝 offload 并提示）

---

## 迭代计划（建议拆解为 I7-1 ~ I7-4）

> 多模态 read_file 与 I2（ToolResultEnvelope 冻结）/I3（offload）强耦合，建议按依赖顺序推进，避免出现“图片塞进 output 造成上下文爆炸”的中间态。

### I7-1：协议冻结与数据结构铺垫（门禁前置）

- 冻结 `read_file` 的 `output` tagged union（text/image）
- 在 ToolResultEnvelope 的 `meta.media` 中补齐 image 最小字段集合（type/mime/size/ref 可选）
- `Message` 增加 `content_blocks`（先不要求 provider 真实发送）

### I7-2：Backend bytes 能力 + read_file(image) 工具输出

- `FilesystemBackend` 增加 `read_bytes`（或 download_files）
- `LocalSandbox` 实现受控 bytes 读取（max_bytes）
- `read_file` 图片分支完成：输出 metadata + tool message blocks（base64）

### I7-3：Runtime/offload/裁剪策略对齐

- 明确并实现图片块的大小策略：
  - 不进入 `output`
  - 超阈值：报错或写入 offload ref（择一）
- 与 `FilesystemRuntimeMiddleware` 的事件字段对齐（记录 media candidates）

### I7-4：Provider/CLI 渲染与端到端用例

- 至少接入一个真实 provider 后：
  - 支持把 `content_blocks` 序列化为 provider 所需的多模态格式
  - 增加 e2e 用例：模型看到图片并能做基础描述/问答
- CLI/ACP：
  - tool message 渲染：图片显示为“image (mime,size)”摘要，必要时支持保存/导出

