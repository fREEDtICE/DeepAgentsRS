---
title: Technical Design and Iteration Plan - Internal LLM Model Levels
scope: iteration
---

## 1. Scope

This document is for the Rust workspace only:

- [`crates/deepagents`](../../crates/deepagents)
- [`crates/deepagents-cli`](../../crates/deepagents-cli)
- [`crates/deepagents-acp`](../../crates/deepagents-acp)

The Python repository is out of scope for this plan.

## 2. Problem

Current DeepAgentsRS provider initialization is exact-config driven.

- CLI `run` accepts exact provider inputs in [`main.rs`](../../crates/deepagents-cli/src/main.rs).
- provider construction happens in [`init.rs`](../../crates/deepagents/src/provider/init.rs).
- the real non-mock provider path today is `openai-compatible`, backed by
  [`OpenAiCompatibleConfig`](../../crates/deepagents/src/provider/openai_compatible/provider.rs).

That is sufficient for direct execution, but it is a poor abstraction for
future orchestration features that need cost/performance tiers.

Required internal levels:

- `lite`: fastest, cheapest, lowest quality
- `normal`: balanced default
- `pro`: strongest quality, highest cost

Important constraint:

- this strategy is not user-facing
- users should only provide provider basics
- future Rust features should request a logical level and let the system resolve
  the exact provider/model/config internally

## 3. Current Rust State

### 3.1 Exact provider init

Current provider initialization is effectively:

```text
provider_id + exact config -> ProviderInitSpec -> build_provider_bundle()
```

The main exact config path today is:

```rust
ProviderInitSpec::OpenAiCompatible { config: OpenAiCompatibleConfig }
```

where `OpenAiCompatibleConfig` currently contains:

- `model`
- `base_url`
- `api_key`
- `multimodal_input_roles`

### 3.2 Why levels should not live in `Provider`

The `Provider` / `LlmProvider` layer already has a clear responsibility:

- request/response conversion
- tool binding
- structured output
- streaming
- capability reporting

`lite | normal | pro` is not a provider wire concern. It is an upstream
selection concern that should resolve before `ProviderInitSpec` is built.

## 4. Goals

### 4.1 Goals

- add an internal model-level abstraction for Rust
- keep `build_provider_bundle()` exact-config based
- support provider-specific mapping for each level
- keep the feature hidden from end users
- make the abstraction reusable by future runtime / agent / ACP features
- preserve deterministic resolution and diagnostics

### 4.2 Non-goals

- no end-user `--model-level` in the initial design
- no requirement that users configure per-level model ids
- no provider auto-benchmarking in this phase
- no silent downgrade from `pro` to `normal` or `lite`
- no changes to mock provider behavior

## 5. Design Summary

Add a new internal selection layer ahead of provider initialization.

```text
feature intent
  -> model level resolver
  -> exact provider selection
  -> exact ProviderInitSpec
  -> build_provider_bundle()
```

The resolver owns:

- `lite | normal | pro`
- provider preference policy
- provider availability checks
- provider-specific exact-model mapping

The provider layer continues to own:

- request execution
- runtime capabilities
- transport and wire format

## 6. Proposed Rust Abstractions

### 6.1 Core enums and structs

Recommended new types in a new module such as
[`provider/catalog.rs`](../../crates/deepagents/src/provider):

```rust
pub enum ModelLevel {
    Lite,
    Normal,
    Pro,
}

pub struct ModelLevelIntent {
    pub level: ModelLevel,
    pub feature_tag: String,
    pub provider_hint: Option<String>,
}

pub struct ProviderBasicConfig {
    pub provider_id: String,
    pub enabled: bool,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
}

pub struct ResolvedProviderSelection {
    pub provider_id: String,
    pub requested_level: ModelLevel,
    pub init_spec: ProviderInitSpec,
    pub diagnostics: ModelLevelResolutionDiagnostics,
}
```

### 6.2 Internal provider catalog

The level mapping should live in an internal Rust catalog, not in user-facing
runtime flags.

Recommended catalog concept:

```rust
pub struct ProviderCatalogEntry {
    pub provider_id: &'static str,
    pub surface: ProviderSurfaceKind,
    pub levels: ProviderLevelMap,
}

pub enum ProviderSurfaceKind {
    OpenAiCompatible,
    // future: AnthropicNative, GeminiNative, etc.
}

pub struct ProviderLevelTarget {
    pub model: &'static str,
    pub base_url_override: Option<&'static str>,
    pub api_key_env_override: Option<&'static str>,
    pub multimodal_input_roles: Option<MultimodalInputRoles>,
}
```

The catalog answers:

- which providers participate in level selection
- how each provider implements `lite`, `normal`, and `pro`
- which exact `ProviderInitSpec` should be produced

### 6.3 Resolution diagnostics

Recommended diagnostics type:

```rust
pub struct ModelLevelResolutionDiagnostics {
    pub feature_tag: String,
    pub requested_level: ModelLevel,
    pub provider_hint: Option<String>,
    pub candidate_providers: Vec<String>,
    pub chosen_provider: String,
    pub chosen_model: String,
}
```

This is for internal logging / tracing / debugging, not mainline end-user UI.

## 7. Resolution Rules

### 7.1 Inputs

Resolver inputs should be:

- `ModelLevelIntent`
- a list or map of configured provider basics
- the internal provider catalog
- optional runtime policy overrides

### 7.2 Resolution algorithm

Recommended algorithm:

1. Receive `ModelLevelIntent`.
2. Build candidate provider list:
   - `provider_hint` first, if present
   - otherwise internal policy order for the requested level
3. For each candidate:
   - check provider is configured/enabled
   - check credentials are present
   - check the catalog defines the requested level
4. Select the first valid candidate.
5. Build exact `ProviderInitSpec` for that candidate.
6. Return `ResolvedProviderSelection`.

### 7.3 Merge precedence

Recommended precedence:

1. provider basic config supplied by the host/app layer
2. internal level target from the catalog
3. explicit internal call-site overrides, if any

That keeps user/provider basics as the transport/auth base while allowing the
system to decide exact tier-specific model settings.

### 7.4 Failure policy

Stable failure cases:

- unknown level
- provider hint not configured
- provider configured but credentials missing
- provider configured but requested level unsupported
- no provider available for requested level

Recommendation:

- fail fast with stable internal error codes
- make downgrade/upgrade explicit future policy, not implicit default behavior

## 8. Integration Points

### 8.1 `crates/deepagents`

Add a new internal resolver module and keep init exact:

- new:
  - `src/provider/catalog.rs`
  - possibly `src/provider/selection.rs`
- existing:
  - [`src/provider/init.rs`](../../crates/deepagents/src/provider/init.rs)

Recommended split:

- `catalog.rs` owns provider-level mappings and resolver logic
- `init.rs` stays responsible for turning exact `ProviderInitSpec` into a real
  `ProviderInitBundle`

That preserves the clean contract already present in `init.rs`.

### 8.2 `crates/deepagents-cli`

For the first iteration, CLI does not need new user-facing flags.

Instead:

- existing exact flags remain as-is
- future internal call sites can use the resolver
- if CLI later needs the feature, it can add a hidden or internal-only path on
  top of the same resolver instead of reimplementing selection logic

### 8.3 `crates/deepagents-acp`

ACP should consume the same resolved exact provider config if/when it needs
internal level selection.

The important point is shared logic:

- one resolver in `crates/deepagents`
- multiple consumers: CLI, ACP, future runtime orchestration

## 9. Provider-Specific Configuration Strategy

### 9.1 User-managed basics

The user/basic host layer should only own provider basics such as:

- whether a provider is enabled
- API key or API-key env
- base URL
- any provider identity needed to connect

This plan does not require end users to configure:

- `lite`
- `normal`
- `pro`
- exact per-level model ids

### 9.2 Internal provider mappings

Example internal catalog entries:

```text
openai:
  lite   -> gpt-5-nano
  normal -> gpt-5.2
  pro    -> o3

anthropic-compatible-or-native:
  lite   -> claude-haiku-*
  normal -> claude-sonnet-*
  pro    -> claude-opus-*
```

The exact provider ids and surfaces should be defined only where DeepAgentsRS
actually supports them. Today that likely means starting with
`openai-compatible`.

### 9.3 Why this matters

Different providers need different exact configuration:

- different model ids
- different base URLs
- potentially different auth env vars
- potentially different multimodal policies

So the level abstraction must resolve into provider-specific exact config, not
just a model string.

## 10. Suggested MVP

Start with a deliberately narrow Rust MVP:

- internal resolver only
- `openai-compatible` only
- no new user-facing CLI flags
- no persistence layer changes
- unit tests around selection logic

This is enough to validate the architecture without overcommitting to product
surface or config format too early.

## 11. Tests

### 11.1 Unit tests in `crates/deepagents`

Add resolver-focused tests for:

- `lite` resolves to expected exact model
- `normal` resolves to expected exact model
- `pro` resolves to expected exact model
- provider hint is respected
- disabled provider is skipped
- missing credentials rejects selection
- unsupported level rejects selection
- diagnostics record chosen provider/model

### 11.2 Integration tests

Once the resolver is wired into a real call site, add integration coverage near:

- [`e2e_openai_compatible.rs`](../../crates/deepagents-cli/tests/e2e_openai_compatible.rs)
- [`provider_openai_http.rs`](../../crates/deepagents/tests/provider_openai_http.rs)

The integration contract should assert that level resolution eventually produces
the same exact HTTP payload shape as manual exact-model configuration.

## 12. Iteration Plan

### Phase 1: Contract

Goal:

- define `ModelLevel`, `ModelLevelIntent`, `ResolvedProviderSelection`

Deliverables:

- new internal selection types
- stable internal error codes
- catalog module skeleton

Acceptance:

- code can express a logical level request without changing provider init

### Phase 2: Resolver

Goal:

- implement provider candidate filtering and exact-spec resolution

Deliverables:

- provider basic config input type
- internal catalog entries
- resolver function producing `ProviderInitSpec`
- unit tests

Acceptance:

- a level request can deterministically resolve to exact `ProviderInitSpec`

### Phase 3: First consumer

Goal:

- wire one Rust internal call site to use the resolver

Recommended first target:

- an internal orchestration path, or
- a hidden CLI path used for testing only

Acceptance:

- at least one real execution path can request `lite | normal | pro` internally

### Phase 4: Provider expansion

Goal:

- support more provider surfaces as DeepAgentsRS grows

Deliverables:

- additional catalog entries
- native-provider mappings if/when new `ProviderInitSpec` variants are added

Acceptance:

- the abstraction remains stable while provider support expands

### Phase 5: Policy evolution

Goal:

- let future features choose levels by intent

Possible additions:

- feature-specific policy tables
- explicit escalation rules
- explicit fallback policies
- internal cost/latency telemetry

## 13. Risks

### 13.1 Mixing selection policy into provider transport

If level logic leaks into `Provider` or `LlmProvider`, the provider layer will
take on orchestration concerns it should not own.

### 13.2 Exposing unstable policy too early

If user-facing flags or config are added before the internal contract settles,
the product surface will freeze too early and become hard to change.

### 13.3 Treating level as only a model string

That would be too weak. Different providers require different exact config, so
the resolver must output full `ProviderInitSpec`-ready data.

## 14. Recommendation

Implement `lite | normal | pro` in DeepAgentsRS as an internal resolver layer
that sits above `ProviderInitSpec` and below future orchestration features.

Keep the boundary strict:

- internal features request a logical level
- the resolver chooses exact provider/model/config
- `build_provider_bundle()` stays exact-config based

That gives Rust a stable cost/performance abstraction without changing the
existing provider execution contract or exposing incomplete strategy controls to
users.
