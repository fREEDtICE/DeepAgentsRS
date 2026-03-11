---
title: Technical Solution and Iteration Plan - Ollama LLM Provider
scope: iteration
---

## 1. Scope

This plan is for the Rust workspace only.

Target crates:

- [`crates/deepagents`](../../crates/deepagents)
- [`crates/deepagents-cli`](../../crates/deepagents-cli)
- [`crates/deepagents-acp`](../../crates/deepagents-acp)

The goal is to add a native `ollama` LLM provider implementation that works for
both:

- local requests to an Ollama daemon
- cloud requests to `ollama.com`

## 2. Confirmed External Constraints

Per Ollama’s official docs:

- the local API base URL is `http://localhost:11434/api`
- the same API is available for cloud requests at `https://ollama.com/api`
- local access requires no authentication
- direct cloud access requires bearer auth with `OLLAMA_API_KEY`
- a local Ollama daemon can also forward cloud-model requests after `ollama signin`

This means DeepAgentsRS should treat "local vs cloud" as transport/config modes
of one provider, not as two separate provider types.

## 3. Current Rust State

The existing provider stack already has the right high-level layering:

- `ProviderInitSpec` in [`provider/init.rs`](../../crates/deepagents/src/provider/init.rs)
- `LlmProvider` in [`provider/llm.rs`](../../crates/deepagents/src/provider/llm.rs)
- a concrete provider module under
  [`provider/openai_compatible`](../../crates/deepagents/src/provider/openai_compatible)

The current real non-mock provider is `openai-compatible`.

Provider initialization is duplicated in:

- CLI: [`deepagents-cli/src/main.rs`](../../crates/deepagents-cli/src/main.rs)
- ACP: [`deepagents-acp/src/server.rs`](../../crates/deepagents-acp/src/server.rs)

That means Ollama should be implemented as:

1. a new provider module in `crates/deepagents`
2. a new `ProviderInitSpec::Ollama`
3. new init-path wiring in CLI and ACP

## 4. Recommendation

Implement Ollama as a **native provider** instead of routing it through the
existing `openai-compatible` provider.

Why:

- the canonical Ollama API for local and cloud is `/api/chat`, not SSE-based
  OpenAI chat completions
- Ollama streaming uses incremental JSON chunks rather than the current
  `text/event-stream` parser
- Ollama has native fields worth preserving:
  - `thinking`
  - `tool_calls`
  - `done`
  - token/duration metrics
  - `format` for structured outputs
- one native implementation can support:
  - local daemon
  - direct cloud API
  - local daemon forwarding cloud models

## 5. Supported Deployment Modes

The provider must support all of these with one config type:

### 5.1 Local daemon

- base URL: `http://localhost:11434/api`
- no auth header
- models can be local models such as `qwen3-coder`

### 5.2 Direct cloud API

- base URL: `https://ollama.com/api`
- bearer auth via `OLLAMA_API_KEY`
- models can be cloud-served models directly available via Ollama Cloud

### 5.3 Local daemon forwarding cloud models

- base URL: local daemon
- no app-managed auth header required
- model name may point to a cloud model
- authentication is handled by the local daemon after `ollama signin`

This third mode matters because it gives users cloud access without forcing
DeepAgentsRS to always talk to `ollama.com` directly.

## 6. Proposed Rust API

### 6.1 New config type

Add a new config struct in a new module:

```rust
pub struct OllamaConfig {
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub multimodal_input_roles: MultimodalInputRoles,
    pub think: Option<OllamaThinkMode>,
    pub keep_alive: Option<String>,
    pub options: serde_json::Map<String, serde_json::Value>,
}
```

Recommended helper enum:

```rust
pub enum OllamaThinkMode {
    Bool(bool),
    Level(String), // "low" | "medium" | "high"
}
```

Notes:

- `base_url` should default to `http://localhost:11434/api`
- `api_key` should remain optional
- `options` gives us an escape hatch for provider-native knobs without
  constantly changing the top-level config

### 6.2 New init spec variant

Extend [`ProviderInitSpec`](../../crates/deepagents/src/provider/init.rs):

```rust
ProviderInitSpec::Ollama { config: OllamaConfig }
```

### 6.3 New provider module

Add:

- `crates/deepagents/src/provider/ollama/mod.rs`
- `crates/deepagents/src/provider/ollama/provider.rs`
- `crates/deepagents/src/provider/ollama/transport.rs`
- `crates/deepagents/src/provider/ollama/wire.rs`

This should mirror the current `openai_compatible` layout.

## 7. Wire Protocol Mapping

### 7.1 Request model

Use Ollama’s native `/api/chat` request shape.

Core request fields to support:

- `model`
- `messages`
- `tools`
- `stream`
- `format`
- `think`
- `options`
- `keep_alive`

### 7.2 Message mapping

Map DeepAgentsRS `Message` to Ollama-native chat messages:

- `role`
- `content`
- image content when supported
- assistant `tool_calls`
- tool result messages
- reasoning/thinking content when possible

Important design choice:

- map DeepAgentsRS `reasoning_content` to Ollama `thinking` where possible
- preserve assistant text + tool calls together, matching the existing
  `ProviderStepOutput` boundary

### 7.3 Tool calling

Ollama supports tool calling with function-style schemas.

Plan:

- reuse the current `FunctionTool` / `ToolsPayload::FunctionTools` concept
- add an Ollama-native tool schema in `wire.rs`
- convert `ToolSpec.input_schema` into Ollama function parameters
- parse Ollama `message.tool_calls[]` into `ProviderToolCall`

### 7.4 Structured output

Ollama supports structured outputs through `format`:

- `"json"`
- or a JSON schema object

Plan:

- map DeepAgentsRS `StructuredOutputSpec` to Ollama `format`
- if strict schema is requested, pass the schema object directly
- keep runtime-side JSON parse behavior unchanged

### 7.5 Streaming

Ollama streaming is not SSE like the current `openai-compatible` transport.

Plan:

- add a new streaming parser for Ollama’s native chunked JSON stream
- aggregate:
  - partial assistant text
  - partial thinking content
  - partial tool calls
  - final usage/metrics
- map these chunks into `LlmEvent`

This is a distinct transport path and should not be forced into the current SSE
parser.

## 8. Transport Layer

### 8.1 Transport trait

Add a dedicated trait:

```rust
pub trait OllamaTransport: Send + Sync {
    async fn chat(
        &self,
        config: &OllamaConfig,
        request: OllamaChatRequest,
    ) -> anyhow::Result<OllamaChatResponse>;

    async fn stream_chat(
        &self,
        config: &OllamaConfig,
        request: OllamaChatRequest,
    ) -> anyhow::Result<OllamaChunkStream>;
}
```

### 8.2 Reqwest transport behavior

`ReqwestOllamaTransport` should:

- POST to `{base_url}/chat`
- send JSON body
- set bearer auth only when `api_key` is present
- parse error bodies into stable provider errors

### 8.3 Local vs cloud handling

Transport logic should not special-case "local" vs "cloud" beyond:

- base URL
- optional auth header

This keeps the provider simple and allows custom/self-hosted remote Ollama
hosts to work too.

## 9. Capability Declaration

The provider should declare capabilities conservatively.

Recommended initial capabilities:

- `supports_streaming = true`
- `supports_tool_calling = true`
- `reports_usage = true`
- `supports_structured_output = true`
- `supports_reasoning_content = true`
- `multimodal` based on config policy

If some fields are not reliably available in all Ollama deployments, prefer
stable degrade-over-claim behavior.

## 10. CLI and ACP Changes

### 10.1 CLI

Extend [`deepagents-cli/src/main.rs`](../../crates/deepagents-cli/src/main.rs):

- accept `--provider ollama`
- reuse existing flags:
  - `--model`
  - `--base-url`
  - `--api-key`
  - `--api-key-env`

Recommended defaults for `ollama`:

- default base URL: `http://localhost:11434/api`
- if `--base-url https://ollama.com/api` and no auth is supplied, try
  `OLLAMA_API_KEY`

### 10.2 ACP

Extend [`deepagents-acp/src/server.rs`](../../crates/deepagents-acp/src/server.rs):

- accept provider `"ollama"`
- mirror the same config resolution as CLI
- return provider diagnostics through existing provider-info paths

### 10.3 Keep config handling shared

Avoid duplicating CLI and ACP wiring logic again.

Recommended follow-up refactor:

- move provider-config resolution helpers into `crates/deepagents`
- let CLI and ACP both call the same helper for:
  - `openai-compatible`
  - `ollama`

This is not required to land the first version, but it should be part of the
plan.

## 11. Error Model

Add stable Ollama-specific error surfaces:

- `ollama_http_error`
- `ollama_invalid_response`
- `ollama_stream_parse_error`
- `ollama_missing_model`
- `ollama_missing_api_key`

Cloud-specific validation:

- direct cloud requests to `https://ollama.com/api` without an API key should
  fail fast in CLI/ACP initialization
- local daemon requests should not require auth from DeepAgentsRS

## 12. Tests

### 12.1 Unit tests in `crates/deepagents`

Add:

- request-building tests
- response parsing tests
- tool-call parsing tests
- structured-output request mapping tests
- NDJSON/chunked-stream parsing tests

Suggested new files:

- `tests/provider_ollama.rs`
- `tests/provider_ollama_http.rs`

### 12.2 Local-mode HTTP tests

Use a mock Axum server to assert:

- request path is `/api/chat`
- no auth header is sent by default
- tool schemas are serialized correctly
- non-stream and stream paths both work

### 12.3 Cloud-mode HTTP tests

Use a mock Axum server to assert:

- base URL can be `https://ollama.com/api`-style equivalent
- bearer auth is sent when configured
- cloud config fails early if auth is missing for direct cloud mode

### 12.4 CLI E2E

Add a new test file similar to
[`e2e_openai_compatible.rs`](../../crates/deepagents-cli/tests/e2e_openai_compatible.rs):

- `e2e_ollama.rs`

Scenarios:

- local non-stream run
- local stream-events run
- direct cloud run with bearer auth
- structured-output run

### 12.5 ACP E2E

Extend:

- [`e2e_phase3_http.rs`](../../crates/deepagents-acp/tests/e2e_phase3_http.rs)

Scenarios:

- provider info for `ollama`
- successful local request
- successful cloud request

## 13. Iteration Plan

### Phase 1: Provider contract and config

Goal:

- add Rust types and init-path plumbing

Deliverables:

- `OllamaConfig`
- `ProviderInitSpec::Ollama`
- module skeleton under `provider/ollama`
- CLI/ACP provider id acceptance

Acceptance:

- code can express exact Ollama provider config for local and cloud modes

### Phase 2: Native wire + transport

Goal:

- implement `/api/chat` sync and streaming transport

Deliverables:

- request/response wire structs
- reqwest transport
- chunked JSON stream parser
- local/cloud auth handling

Acceptance:

- unit and HTTP tests cover local non-auth and cloud bearer-auth paths

### Phase 3: `LlmProvider` implementation

Goal:

- map native Ollama API into DeepAgentsRS provider abstractions

Deliverables:

- `OllamaProvider`
- tool conversion
- structured output mapping
- reasoning/thinking mapping
- `LlmEvent` streaming aggregation

Acceptance:

- provider returns stable `ProviderStepOutput` for sync and stream paths

### Phase 4: CLI and ACP integration

Goal:

- expose `ollama` through existing run surfaces

Deliverables:

- CLI `--provider ollama`
- ACP request support for `provider = "ollama"`
- provider diagnostics wired through existing output paths

Acceptance:

- both CLI and ACP can run local and direct cloud Ollama requests

### Phase 5: Validation hardening

Goal:

- close compatibility gaps and stabilize behavior

Deliverables:

- better error classification
- stricter auth validation for cloud direct mode
- more wire fixtures
- optional shared provider-config resolver refactor

Acceptance:

- local request path and cloud request path are both regression-tested

## 14. Recommendation

Implement Ollama as a native provider with one config and transport model that
supports:

- local daemon
- direct cloud API
- local daemon forwarding cloud models

Keep the boundary consistent with the rest of DeepAgentsRS:

- provider selection and init stay exact-config based
- `OllamaProvider` implements `LlmProvider`
- CLI and ACP reuse the same provider-init semantics

This gives DeepAgentsRS full Ollama support without forcing the provider
through an incompatible OpenAI/SSE transport shape.
