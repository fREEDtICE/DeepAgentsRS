# deepagents (Rust)

该目录是 `py/` 版本在 Rust 下的对应实现。目标是提供与 Python 版本一致的核心抽象（backends、middleware、tools、subagents），并逐步补齐 CLI 与 ACP server。

## Workspace 结构

- `crates/deepagents`：核心库
- `crates/deepagents-cli`：命令行入口（最小可用）
- `crates/deepagents-acp`：ACP server（最小可用）

## 开发

```bash
cargo test
```
