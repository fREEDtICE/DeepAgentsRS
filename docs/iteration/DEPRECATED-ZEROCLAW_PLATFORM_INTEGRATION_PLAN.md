# DEPRECATED! DO NOT READ!

# DeepAgentsRS + ZeroClaw Platform Integration Plan

## Purpose

This document captures the recommended integration strategy for combining:

- `DeepAgentsRS` as the agent engine and runtime core
- selected `ZeroClaw` features as the outer platform shell

The goal is to keep DeepAgentsRS as the source of truth for agent behavior while still benefiting from community-maintained platform capabilities such as channels, cron, daemon supervision, gateway/web apps, and operational tooling.

## Core Decision

Do not migrate DeepAgentsRS into ZeroClaw's internal agent implementation.

Instead:

- keep `DeepAgentsRS` as the core engine
- keep or adopt `ZeroClaw` as the outer shell
- replace only the execution junctions where ZeroClaw transport code hands off to its own agent loop

This is a shell-core architecture:

- shell: transport, auth, rate limits, scheduling, daemon, delivery
- core: tool loop, middleware, subagents, HITL, runtime state, workspace memory injection

## Why This Strategy

### What DeepAgentsRS already is

DeepAgentsRS is already shaped like an engine/runtime library:

- typed runtime builder
- runtime middleware chain
- structured run output
- streaming runtime events
- stateful execution core

Relevant code:

- `crates/deepagents/src/agent.rs`
- `crates/deepagents/src/runtime/protocol.rs`
- `crates/deepagents/src/runtime/events.rs`

### What ZeroClaw is strongest at

ZeroClaw is strongest as a platform shell:

- channels
- daemon supervision
- cron scheduling
- HTTP gateway
- transport-facing auth and delivery

Relevant code:

- `../zeroclaw/src/channels/mod.rs`
- `../zeroclaw/src/channels/traits.rs`
- `../zeroclaw/src/daemon/mod.rs`
- `../zeroclaw/src/cron/scheduler.rs`
- `../zeroclaw/src/gateway/mod.rs`

### The main problem in current ZeroClaw

Current ZeroClaw mixes shell concerns and engine concerns in the same flow:

- channel message processing eventually calls `run_tool_call_loop(...)`
- gateway tool-enabled chat calls `crate::agent::process_message(...)`
- cron agent jobs call `crate::agent::run(...)`

That means an in-place replacement of the ZeroClaw agent loop would create a long-term merge and maintenance problem.

## Architecture Summary

Target shape:

```text
Channel / Webhook / Cron
-> ZeroClaw-derived shell
-> AgentEngine boundary
-> DeepAgentsRunner
-> DeepAgentsRS runtime
-> event bridge
-> delivery / persistence / logs
```

Not target shape:

```text
DeepAgentsRS inside ZeroClaw internals
```

The correct relationship is:

```text
ZeroClaw shell around DeepAgentsRS
```

## Current Call Chain

### 1. Channels

Current flow:

```text
start_channels()
-> provider + memory + tools assembly
-> process_channel_message()
-> runtime commands / route resolution / autosave / history
-> memory context injection
-> draft typing / reactions
-> run_tool_call_loop(...)
-> send or finalize response
```

Current relevant locations:

- `../zeroclaw/src/channels/mod.rs` (`start_channels`, `process_channel_message`, `run_tool_call_loop` call path)

### 2. Gateway / Webhook

Current flow:

```text
handle_webhook()
-> auth / rate limit / parse request
-> run_gateway_chat_with_tools()
-> crate::agent::process_message(config, message)
-> response
```

Current relevant locations:

- `../zeroclaw/src/gateway/mod.rs` (`run_gateway_chat_with_tools`, `handle_webhook`)

### 3. Cron

Current flow:

```text
cron scheduler tick
-> execute_agent_job()
-> crate::agent::run(...)
-> persist run result
-> optional delivery
```

Current relevant locations:

- `../zeroclaw/src/cron/scheduler.rs` (`execute_agent_job`)

## Recommended Call Chain

### Common pattern

All entrypoints should converge on the same engine boundary:

```text
external input
-> platform shell logic
-> normalized InboundTurn
-> AgentEngine::run_turn(...)
-> DeepAgentsRunner
-> DeepAgentsRS StreamingRuntime
-> RunEventSink bridge
-> final result + persistence + delivery
```

### Runtime event support already exists

DeepAgentsRS already has the right core seams:

- `StreamingRuntime::run_with_events(...)`
- `RunEvent`
- `RunEventSink`

Relevant locations:

- `crates/deepagents/src/runtime/protocol.rs`
- `crates/deepagents/src/runtime/events.rs`

## Integration Boundary

Introduce a transport-neutral boundary owned by DeepAgentsRS-side integration code.

Example shape:

```rust
#[async_trait]
pub trait AgentEngine: Send + Sync {
    async fn run_turn(
        &self,
        input: InboundTurn,
        sink: &mut dyn RunEventSink,
    ) -> anyhow::Result<TurnResult>;
}
```

Recommended transport-neutral request shape:

```rust
pub struct InboundTurn {
    pub session_id: String,
    pub source: TurnSource,
    pub principal: Principal,
    pub user_text: String,
    pub reply_target: Option<String>,
    pub thread_id: Option<String>,
    pub route: RouteIntent,
    pub trace: TraceContext,
    pub metadata: serde_json::Value,
}
```

Recommended structured shell-provided identity:

```rust
pub struct Principal {
    pub subject_id: String,
    pub roles: Vec<String>,
    pub authn_method: AuthnMethod,
    pub permissions: Vec<String>,
}
```

Recommended tracing handoff:

```rust
pub struct TraceContext {
    pub trace_id: String,
    pub span_id: Option<String>,
}
```

Recommended result shape:

```rust
pub struct TurnResult {
    pub final_text: String,
    pub interrupted: bool,
    pub state_changed: bool,
    pub interrupt: Option<InterruptPayload>,
}
```

The shell should not know how DeepAgentsRS performs tool loops, provider steps, or middleware execution. It should only know how to:

- normalize inbound transport data
- open the correct event sink
- call the engine
- deliver the result

Security-sensitive authorization context should not be hidden only inside loose `metadata`.
The shell should pass verified identity and permission context explicitly through `Principal`.

In the first implementation, prefer reusing the existing DeepAgentsRS `RunEvent` /
`RunEventSink` contract instead of inventing a second boundary event enum too early.
Shell-specific action mapping can happen in adapters above that layer.

## Data Ownership

### ZeroClaw shell owns

- channel listeners and transport implementations
- webhook parsing and HTTP auth
- pairing / bearer token logic
- rate limiting and idempotency
- daemon process supervision
- cron schedule calculation and run history
- draft message delivery and typing indicators
- thread IDs, reply targets, sender identity, route overrides

### DeepAgentsRS owns

- runtime loop
- tool orchestration
- middleware ordering
- HITL interrupts and resumes
- subagents
- summarization
- prompt assembly internal to the engine
- state mutation during a run

### Shared platform services exposed through adapters

- semantic memory
- cron/job mutation tools
- notification or channel delivery tools
- web fetch / browser / HTTP capabilities if retained

Adapters should remain trait-first integration points. DeepAgentsRS should depend on
capability traits, not on ZeroClaw concrete types.

## End-to-End Data Flow

### 1. Channel flow

Recommended flow:

```text
ChannelMessage
-> transport hooks and command handling
-> derive session_id
-> create ChannelEventSink
-> AgentEngine::run_turn(InboundTurn, sink)
-> map streaming events to draft updates
-> finalize message
-> persist session state
```

Concrete ownership split:

- keep `Channel` implementations and channel message bus from ZeroClaw
- keep reactions, typing, draft handling, cancellation, route selection
- replace direct call to `run_tool_call_loop(...)`

Mapping idea:

- `RunEvent::AssistantTextDelta` -> `update_draft(...)`
- `RunEvent::Interrupt` -> approval prompt or channel-side action
- `RunEvent::RunFinished` -> `finalize_draft(...)` or `send(...)`

### 2. Gateway / web app flow

Recommended flow:

```text
HTTP request
-> auth / rate limit / parse body
-> derive session_id
-> create GatewayEventSink
-> AgentEngine::run_turn(InboundTurn, sink)
-> publish SSE / websocket updates
-> return final response
```

Concrete ownership split:

- keep HTTP gateway, auth, rate limits, pairing, SSE, websocket fanout
- replace `crate::agent::process_message(...)`

### 3. Cron flow

Recommended flow:

```text
CronJob
-> scheduler security checks
-> derive session_id from job policy
-> create CronEventSink or Noop sink
-> AgentEngine::run_turn(InboundTurn, sink)
-> record output in cron run history
-> optional delivery
```

Concrete ownership split:

- keep cron DB, schedule logic, run history, delivery policy
- replace `crate::agent::run(...)`

## Memory Model

Two memory systems should coexist because they solve different problems.

### 1. Workspace memory

Owned by DeepAgentsRS runtime/middleware.

Purpose:

- inject workspace-local instructions and persistent repo context
- typically sourced from files such as `AGENTS.md`

Characteristics:

- deterministic
- file-oriented
- tied to workspace/runtime context

Relevant location:

- `crates/deepagents/src/runtime/memory_middleware.rs`

### 2. Semantic long-term memory

Owned by the outer platform service.

Purpose:

- store durable facts, preferences, notes, conversation-derived knowledge
- support retrieval across conversations and transports

Characteristics:

- backend-oriented
- semantic or search-oriented
- not the same thing as workspace prompt injection

Relevant location:

- `../zeroclaw/src/memory/traits.rs`

### Recommended memory flow

Before run:

- optional semantic memory recall can enrich the initial run context

During run:

- DeepAgentsRS loads workspace memory via middleware
- DeepAgentsRS may access semantic memory through adapter-exposed tools

After run:

- platform may persist durable facts to semantic memory
- DeepAgentsRS runtime state is persisted separately as session state

Do not collapse these two concepts into one trait.

## Session Model

Recommended session ownership:

- shell owns transport/session identity
- engine owns runtime execution state

Suggested session ID conventions:

- channels: `channel:<channel_name>:<sender>:<thread_or_default>`
- webhook/chat: `http:<client_or_pairing_identity>:<conversation_id>`
- cron main session: `cron:<job_id>`
- cron isolated run: `cron:<job_id>:<run_timestamp>`

Persist runtime state by `session_id`.

## Provider and Route Selection

The shell may still own route selection UX:

- current provider override
- current model override
- sender-specific route overrides

But the shell should pass route intent to the engine, not execute the model loop itself.

Recommended split:

- shell owns `RouteIntent { provider_id, model_id }`
- engine/runner factory builds the correct DeepAgentsRS provider adapter from that intent

This keeps transport code independent from provider internals.

Suggested provider factory boundary:

```rust
pub struct RouteIntent {
    pub provider_id: String,
    pub model_id: String,
}

pub trait ProviderFactory {
    fn create(&self, intent: &RouteIntent) -> anyhow::Result<Box<dyn Provider>>;
}
```

## Configuration Boundary

`DeepAgentsRunner` should not depend directly on `zeroclaw::Config`.

Instead:

- shell reads and owns `zeroclaw::Config`
- shell maps the needed values into a dedicated DeepAgents-facing config struct
- runner receives only the mapped config it needs

Recommended pattern:

```rust
pub struct DeepAgentsRunnerConfig {
    pub workspace_root: PathBuf,
    pub runtime: RuntimeConfig,
    pub tool_policy: ToolPolicyConfig,
    pub memory: EngineMemoryConfig,
}
```

This prevents the shell-core boundary from collapsing back into a direct repo-level dependency.

## What Must Change In Practice

This strategy still requires refactoring, but only at the boundary.

It does not mean "use ZeroClaw untouched".

It means:

- do not maintain a full behavioral fork of ZeroClaw's agent internals
- do not let every feature module call ZeroClaw agent code directly
- introduce one stable engine seam and route channels/gateway/cron through it

Concrete replacements:

Current channel path:

```rust
run_tool_call_loop(...).await
```

Target channel path:

```rust
engine.run_turn(inbound_turn, &mut channel_sink).await
```

Current gateway path:

```rust
crate::agent::process_message(config, message).await
```

Target gateway path:

```rust
engine.run_turn(inbound_turn, &mut gateway_sink).await
```

Current cron path:

```rust
crate::agent::run(config.clone(), Some(prompt), ...)
```

Target cron path:

```rust
engine.run_turn(inbound_turn, &mut cron_sink).await
```

## Migration Order

Use the smallest-risk entrypoint first.

### Phase 1: cron

Reason:

- narrowest call site
- easiest to validate
- no streaming UX dependency

Goal:

- prove that a ZeroClaw shell entrypoint can call DeepAgentsRS end to end

### Phase 2: gateway tool-enabled path

Reason:

- HTTP lifecycle is easier to test than full multi-channel behavior
- enables SSE/web event integration through `RunEventSink`

Goal:

- prove streaming + request normalization + result delivery

### Phase 3: channels

Reason:

- most moving parts
- typing, drafts, reactions, cancellation, route overrides

Goal:

- replace the last major execution junction without changing transport behavior

## Suggested Crate Layout

One reasonable direction:

```text
DeepAgentsRS/
  crates/
    deepagents/                   # existing core engine
    deepagents-platform/          # neutral platform boundary types
    deepagents-zeroclaw-compat/   # adapters for channels/gateway/cron/memory
```

Possible contents:

- `deepagents-platform`
  - `InboundTurn`
  - `TurnResult`
  - `TurnEvent`
  - `AgentEngine`
  - `SessionStore`

- `deepagents-zeroclaw-compat`
  - channel event sink adapter
  - gateway event sink adapter
  - cron event sink adapter
  - semantic memory adapter
  - provider route adapter

## Risks

### 1. ZeroClaw shell files are still broad

`channels/mod.rs` currently mixes transport logic, prompt building, memory access, provider selection, and execution. This means the first extraction pass must untangle concerns carefully.

### 2. Memory semantics can drift

If semantic memory and workspace memory are blurred together, prompt behavior will become unpredictable and maintenance cost will rise.

### 3. Provider ownership can become ambiguous

If the shell both selects and instantiates providers while the engine also wants provider control, duplication will appear quickly. Route intent should stay in the shell; provider execution should be owned by the engine/runner factory.

### 4. Authorization and trace context can disappear at the boundary

If `InboundTurn` only carries free-form metadata, security and observability become fragile. Verified identity, permissions, and trace context should cross the shell-core boundary explicitly.

## Non-Goals

This plan does not require:

- rewriting all ZeroClaw modules
- keeping ZeroClaw's current agent loop
- merging the two memory systems
- adopting all ZeroClaw tools or provider internals wholesale

## Immediate Next Steps

1. Define `InboundTurn`, `TurnResult`, and `AgentEngine`.
2. Implement a `DeepAgentsRunner` that builds a DeepAgentsRS runtime per turn or per session.
3. Replace the cron execution call site first.
4. Add event sink adapters for gateway and channels.
5. Decide which ZeroClaw services remain shell-owned and which become engine-exposed adapters.

## Final Principle

Own the core, borrow the shell.

DeepAgentsRS should remain the source of truth for agent behavior.

ZeroClaw-derived code should remain the source of truth for platform transport and operations.
