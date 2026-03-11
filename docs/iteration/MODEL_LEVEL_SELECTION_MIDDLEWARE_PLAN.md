---
title: Technical Solution and Iteration Plan - Model Level Selection Middleware
scope: iteration
---

## 1. Scope

This plan is Rust-only and builds on the internal model-level resolver already
introduced in [`catalog.rs`](../../crates/deepagents/src/provider/catalog.rs).

Relevant runtime/provider surfaces:

- [`runtime/protocol.rs`](../../crates/deepagents/src/runtime/protocol.rs)
- [`runtime/resumable_runner.rs`](../../crates/deepagents/src/runtime/resumable_runner.rs)
- [`runtime/assembly.rs`](../../crates/deepagents/src/runtime/assembly.rs)
- [`runtime/simple.rs`](../../crates/deepagents/src/runtime/simple.rs)
- [`provider/init.rs`](../../crates/deepagents/src/provider/init.rs)

## 2. Problem

We now have an internal logical level abstraction:

- `lite`
- `normal`
- `pro`

But there is still no runtime hook that can choose the proper level for a given
run based on:

- the user query
- current state/context
- tool/structured-output requirements
- task depth or feature metadata

Current gap:

- `RuntimeMiddleware::before_run()` can mutate messages/state, but it cannot
  swap the active provider
- `ResumableRunner` stores a fixed `provider: Arc<dyn Provider>`
- provider selection is frozen before the run starts

So the current middleware shape is not sufficient for a "choose model level for
this run" feature.

## 3. Current Runtime Boundary

### 3.1 What exists today

`ResumableRunner` currently does:

1. normalize initial messages
2. execute all `before_run()` runtime middlewares once
3. enter the provider/tool loop

That means the correct insertion point already exists conceptually:

- after `before_run()`
- before the first provider step

### 3.2 Why `before_run()` is not enough

`before_run()` currently returns only `Vec<Message>`.

It cannot cleanly express:

- choose `ModelLevel`
- resolve exact provider/model
- replace the active `Provider`
- record provider-resolution diagnostics

Using `state.extra` alone is not enough, because the runner still owns a fixed
provider instance.

## 4. Design Goals

### 4.1 Goals

- run once before the agent call loop starts
- inspect the user query and current run context
- choose the appropriate logical model level for this run
- resolve that level into exact provider/model/config
- swap the active provider before step 0
- keep the feature internal and deterministic
- preserve current exact-provider runtime path when no selector is installed

### 4.2 Non-goals

- no user-facing `--model-level` in this phase
- no mid-run provider switching
- no re-selection on HITL resume unless explicitly designed later
- no model-generated self-routing

## 5. Proposed Architecture

### 5.1 New lifecycle hook

Add a new runtime middleware hook that runs once after `before_run()` and before
the first provider step.

Recommended addition to `RuntimeMiddleware`:

```rust
async fn before_agent_loop(
    &self,
    _ctx: &mut RunStartContext<'_>,
) -> anyhow::Result<Option<RunStartMutation>> {
    Ok(None)
}
```

This keeps `before_run()` for message/state preparation and adds a dedicated
hook for provider/model selection.

### 5.2 New context object

Recommended context passed to the hook:

```rust
pub struct RunStartContext<'a> {
    pub messages: &'a [Message],
    pub state: &'a mut AgentState,
    pub root: &'a str,
    pub mode: ExecutionMode,
    pub task_depth: usize,
    pub tool_choice: &'a ToolChoice,
    pub structured_output: Option<&'a StructuredOutputSpec>,
    pub tool_specs: &'a [ToolSpec],
    pub skill_names: &'a [String],
    pub active_provider_id: Option<&'a str>,
}
```

Recommended helper methods:

- `latest_user_message() -> Option<&Message>`
- `latest_user_query() -> Option<&str>`

This gives the selector stable, pre-loop context without exposing runtime
internals it does not need.

### 5.3 New mutation type

Recommended mutation type:

```rust
pub enum RunStartMutation {
    ProviderSelection(ResolvedProviderSelection),
}
```

This is intentionally narrow. For this middleware, the only important mutation
is selecting the run-scoped provider.

### 5.4 Dedicated middleware

Add a new middleware implementation:

```rust
pub struct ModelLevelSelectionMiddleware {
    policy: Arc<dyn ModelLevelPolicy>,
    provider_configs: Vec<ProviderBasicConfig>,
    catalog: Vec<ProviderCatalogEntry>,
}
```

This middleware should:

1. inspect `RunStartContext`
2. derive a `ModelLevelDecision`
3. turn it into `ModelLevelIntent`
4. call `resolve_model_level_selection_with_catalog(...)`
5. return `RunStartMutation::ProviderSelection(...)`

## 6. Policy Layer

### 6.1 Separate policy from lifecycle

Do not bake heuristics directly into the middleware type.

Recommended separate trait:

```rust
#[async_trait]
pub trait ModelLevelPolicy: Send + Sync {
    async fn choose_level(
        &self,
        ctx: &ModelLevelPolicyContext<'_>,
    ) -> anyhow::Result<ModelLevelDecision>;
}
```

Recommended decision type:

```rust
pub struct ModelLevelDecision {
    pub level: ModelLevel,
    pub feature_tag: String,
    pub provider_hint: Option<String>,
    pub reason: Option<String>,
}
```

This separation is important because:

- the middleware owns runtime lifecycle integration
- the policy owns heuristics and routing logic

### 6.2 Recommended V1 policy shape

Start with deterministic rules, not learned routing.

Recommended V1 decision inputs:

- latest user query text
- task depth
- whether strict structured output is requested
- tool names exposed to the run
- feature tag supplied by the caller

Recommended V1 rules:

- default to `normal`
- choose `lite` for low-risk utility flows such as summarization or lightweight
  maintenance tasks identified by feature tag
- choose `pro` only for explicit high-complexity signals
  - large/complex design prompt
  - explicit compare/analyze/debug/root-cause style requests
  - deep task depth or explicit escalation feature tag

The key is predictability, not sophistication.

## 7. Runner Changes

### 7.1 Run-scoped provider replacement

`ResumableRunner` needs to support replacing the active provider before step 0.

Minimum change:

- keep the existing `provider` field
- after `before_run()`, execute `before_agent_loop()`
- if a provider selection mutation is returned:
  - build a bundle from `ResolvedProviderSelection`
  - assign `self.provider = bundle.provider`

### 7.2 Active provider metadata

The runner should also track the selected provider identity for diagnostics.

Recommended new fields on `ResumableRunner`:

```rust
active_provider_id: Option<String>,
active_provider_diagnostics: Option<ProviderDiagnostics>,
```

This matters because after selection, the provider used for the run may differ
from whatever the caller originally wired in.

### 7.3 One-time execution

The selector should run only once per fresh run.

Recommended rule:

- run during the `!self.initialized` path only
- persist selection metadata into `state.extra`
- skip re-selection on resume

That keeps HITL resume deterministic.

## 8. State, Trace, and Events

### 8.1 State storage

Recommended state key:

- `_model_level_selection`

Suggested value:

```json
{
  "requested_level": "normal",
  "feature_tag": "agent_turn",
  "provider_id": "openai-compatible",
  "model": "gpt-4o",
  "reason": "default_normal"
}
```

### 8.2 Run events

Recommended new event:

```rust
RunEvent::ModelLevelSelected {
    requested_level: ModelLevel,
    provider_id: String,
    model: String,
    feature_tag: String,
}
```

This is useful for internal debugging and event-based tests.

### 8.3 Trace output

Recommended trace insertion:

- `trace.model_level_selection`

This should mirror the state event in a stable form without relying on
middleware-specific log parsing.

## 9. Interaction with Existing Middlewares

### 9.1 Prompt caching

This is the most important interaction.

`PromptCachingMiddleware` stores provider-specific cache options in state. If
the active provider changes after selection, the cached provider id must be
updated before the first provider step.

Recommended behavior after provider selection:

- update `_prompt_cache_options.provider_id` if present

Without this, cache partitioning and diagnostics may point to the wrong
provider.

### 9.2 Memory / skills / filesystem runtime

These middlewares already enrich messages/state in `before_run()`.

That is a good fit:

- selector can inspect the enriched messages after those middlewares run
- no provider switching needs to happen earlier than that

### 9.3 Summarization

`SummarizationMiddleware` currently acts in `before_provider_step()`, not
`before_run()`.

That means the selected provider is already active before summarization logic
starts, which is the correct behavior if summarization later needs to look at
provider-specific budgets or diagnostics.

## 10. Middleware Ordering

Add a dedicated slot in [`assembly.rs`](../../crates/deepagents/src/runtime/assembly.rs):

```rust
RuntimeMiddlewareSlot::ModelSelection
```

Recommended position:

- after `Subagents`
- before `Summarization`

Even if `before_agent_loop()` is a separate phase from `before_run()`, the slot
is still useful to:

- make the middleware first-class in assembly
- prevent duplicate built-in selectors
- keep future ordering predictable

## 11. Proposed File Changes

### 11.1 Core runtime

- [`runtime/protocol.rs`](../../crates/deepagents/src/runtime/protocol.rs)
  - add `RunStartContext`
  - add `RunStartMutation`
  - extend `RuntimeMiddleware`
- [`runtime/resumable_runner.rs`](../../crates/deepagents/src/runtime/resumable_runner.rs)
  - execute new hook before step 0
  - swap active provider when selected
  - store diagnostics in state/trace/events
- [`runtime/events.rs`](../../crates/deepagents/src/runtime/events.rs)
  - add `ModelLevelSelected`
- [`runtime/assembly.rs`](../../crates/deepagents/src/runtime/assembly.rs)
  - add `ModelSelection` slot

### 11.2 New middleware module

Add:

- `runtime/model_level_selection_middleware.rs`

This module should contain:

- `ModelLevelSelectionMiddleware`
- `ModelLevelPolicy`
- `ModelLevelDecision`
- `ModelLevelPolicyContext`
- default deterministic policy

### 11.3 Runtime exports

Update [`runtime/mod.rs`](../../crates/deepagents/src/runtime/mod.rs) to export the
new middleware and types.

## 12. Test Plan

### 12.1 Unit tests

Add focused tests for:

- policy chooses `normal` by default
- policy chooses `lite` for configured low-cost feature tags
- policy chooses `pro` for explicit complex requests
- middleware writes `_model_level_selection` into state
- middleware returns exact `ResolvedProviderSelection`

### 12.2 Runner integration tests

Add tests proving:

- `before_agent_loop()` runs exactly once on fresh run
- selected provider replaces the original provider before step 0
- provider selection is preserved across resume
- prompt cache provider id is updated after selection

Suggested new test file:

- `crates/deepagents/tests/model_level_selection_middleware.rs`

### 12.3 Provider-path integration tests

Use or extend:

- [`provider_model_levels.rs`](../../crates/deepagents/tests/provider_model_levels.rs)
- [`provider_openai_http.rs`](../../crates/deepagents/tests/provider_openai_http.rs)

Goal:

- prove the middleware-selected provider produces the exact expected downstream
  provider request

## 13. Iteration Plan

### Phase 1: Runtime hook and contracts

Goal:

- create the new pre-loop hook and runner plumbing

Deliverables:

- `RunStartContext`
- `RunStartMutation`
- `RuntimeMiddleware::before_agent_loop()`
- runner support for provider override

Acceptance:

- a test middleware can replace the provider before step 0

### Phase 2: Selection middleware skeleton

Goal:

- add the actual middleware and wire it to the resolver

Deliverables:

- `ModelLevelSelectionMiddleware`
- `ModelLevelPolicy` trait
- default pass-through or fixed-level policy for bootstrap

Acceptance:

- middleware can choose `normal` and resolve exact `ProviderInitSpec`

### Phase 3: Deterministic V1 policy

Goal:

- choose level using query/context heuristics

Deliverables:

- rule-based policy
- policy test matrix
- selection diagnostics in state and events

Acceptance:

- simple tasks route to `lite`
- default tasks route to `normal`
- explicit complex tasks route to `pro`

### Phase 4: Middleware interactions and observability

Goal:

- make the feature stable with existing runtime middlewares

Deliverables:

- prompt cache provider-id update
- model-selection run event
- trace/state diagnostics

Acceptance:

- selected provider is visible in trace/events
- prompt cache state reflects the selected provider

### Phase 5: First real internal consumer

Goal:

- let one real Rust internal feature use the middleware

Examples:

- agent turn routing
- summarization routing
- subagent escalation routing

Acceptance:

- one production path no longer hardcodes exact provider:model ids

## 14. Recommendation

Implement the selector as a dedicated **pre-loop runtime middleware hook**,
backed by a separate `ModelLevelPolicy` trait and the existing internal
model-level resolver.

That design fits the current Rust runtime architecture because it:

- respects the existing `before_run()` contract
- swaps the provider exactly once before step 0
- keeps provider execution exact-config based
- gives future orchestration features a stable place to choose `lite`,
  `normal`, or `pro`
