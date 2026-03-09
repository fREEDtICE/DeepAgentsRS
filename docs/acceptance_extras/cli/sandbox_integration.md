---
title: Extras CLI E2E - Sandbox/远程执行集成（可选）
scope: extras
---

## 1. 端到端效果

当 CLI 支持接入远程 sandbox/provider（例如 Daytona/Modal/Runloop 等）时，端到端验收关注：

- 能力发现：CLI 能识别当前 sandbox 是否支持 execute/upload/download
- 安全边界：禁止在错误配置下执行高风险命令
- 可靠性：网络失败可诊断、可重试、不会破坏本地会话状态

## 2. 验收环境

- 提供一个本地 mock sandbox server（CI 可启动），实现最小 API：
  - execute
  - upload_files/download_files
  - ls/read/write/edit
- CLI 通过配置指定 sandbox endpoint（例如 `--sandbox-url`）

## 3. E2E 场景（建议）

### CSI-01：execute 能力随 sandbox 可用性切换

给定：

- mock sandbox 标记支持 execute

当：CLI 运行一个包含 execute 的任务

则：

- execute tool 可用并成功返回输出

当：

- mock sandbox 关闭 execute 能力

则：

- execute tool 不暴露或调用返回明确错误

### CSI-02：文件上传下载链路

给定：

- 本地准备一个文件 `in.txt`

当：

- CLI 将其上传到 sandbox，再在 sandbox 内读取并回显内容

则：

- 上传/读取成功，内容一致

### CSI-03：网络失败的可诊断与恢复

给定：

- sandbox 在中途断连

当：CLI 执行任务

则：

- 给出明确错误（包含请求类型与可重试建议）
- 不破坏本地 artifacts/checkpoint，可再次运行恢复

## 4. 通过标准

- 若 Extras 宣称支持 sandbox 集成，则 CSI-01/02/03 必须通过

