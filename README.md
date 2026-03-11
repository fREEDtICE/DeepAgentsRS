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

## 推荐运行时装配

`DeepAgent` 负责承载 backend、tools 和 middleware；真正可执行的对话循环通过显式 runtime 装配完成。

```rust
use std::sync::Arc;

use deepagents::provider::mock::{MockProvider, MockScript, MockStep};
use deepagents::runtime::Runtime;
use deepagents::types::Message;

let agent = deepagents::create_deep_agent("/tmp/workspace")?;
let provider = Arc::new(MockProvider::from_script(MockScript {
    steps: vec![MockStep::FinalText {
        text: "done".to_string(),
    }],
}));

let runtime = agent
    .runtime(provider)
    .with_root("/tmp/workspace")
    .build()?;

let out = runtime
    .run(vec![Message {
        role: "user".to_string(),
        content: "hello".to_string(),
        content_blocks: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        status: None,
    }])
    .await;
```

`DeepAgent.run()` 仍保留为向后兼容的默认空响应行为；推荐新代码统一走 `DeepAgent::runtime(provider).with_root(...).build()` 的显式运行时装配路径。
