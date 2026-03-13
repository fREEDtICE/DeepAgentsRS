# Cross-Platform Prompt Cache Design for DeepAgentRS

## Summary
- Scope: evolve DeepAgentRS prompt caching to match the cross-platform guide, not redesign the whole agent/runtime stack.
- Keep the current execution spine intact: [prompt_cache_runtime.rs](/Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/runtime/prompt_cache_runtime.rs), [resumable_runner.rs](/Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/runtime/resumable_runner.rs), and [provider/llm.rs](/Users/bytedance/Documents/Dev/deepagents-rs/DeepAgentsRS/crates/deepagents/src/provider/llm.rs) remain the main integration points.
- Current baseline is good enough to extend, not replace: DeepAgentRS already has working in-memory L1/L2 prompt caching, trace redaction, and passing prompt-cache tests. The remaining work is not optional polish: cache hashes must be derived from the exact provider payload sent on the wire, and native prefix caching must work even when L2 response caching is disabled.
- Core design rule: cache behavior must be based on the exact final token prefix sent to the provider. Multiple `system` messages are only an encoding choice; they are not the cache primitive.

## Key Changes
- Introduce a provider-owned `PromptCachePlan` generated from `AgentProviderRequest` after all runtime middlewares have mutated messages.
- `PromptCachePlan` is valid only if it is computed from the final provider payload after all provider-specific normalization.
- Forbidden pattern: building `prompt_cache_plan()` from intermediate `Message` or `ChatRequest`, then mutating the outbound payload later via system flattening, top-level system extraction, or prompt-guided instruction insertion.
- `PromptCachePlan` must return three canonical views:
  - `l0_view`: provider/model/tool-choice/runtime knobs that affect provider behavior.
  - `l1_view`: stable prefix view, consisting of contiguous leading `system` or `developer` instructions plus tool schema and any provider-added fixed tool-guidance instructions.
  - `l2_view`: dynamic suffix view, consisting of all remaining user/assistant/tool messages plus `_summarization_event` if present.
- Canonicalization must happen on the provider-mapped payload, not raw `Message`. If a provider merges system into user or uses a top-level system field, the merge happens first, then hashing happens on that mapped view.
- Replace the current placeholder L1 value `()` with `PromptPrefixArtifact`, which stores the L1 hash, strategy, creation time, and an optional opaque provider-native cache handle.
- Keep L2 as optional full response caching for deterministic or mock flows only. It stays off by default and must continue to short-circuit the provider exactly as it does today.
- Extend cache execution flow in the runtime:
  1. build `PromptCachePlan`
  2. lookup/insert L1
  3. always apply `ProviderPromptCacheHint` before provider execution when `native != off`, regardless of whether `l2` is enabled
  4. execute provider and collect provider events
  5. always call `observe_prompt_cache_result(output, provider_events)` and refresh the L1 artifact when a new handle is returned
  6. optionally lookup/insert L2 for full-response caching after the native/L1 path has run
  7. collect local and provider-native cache observations into one trace stream
- Add provider strategy modes:
  - `none`
  - `stable_prefix`
  - `cache_control`
  - `context_cache`
  - `common_prefix`
  - `kv_reuse`
- For the current codebase:
  - `mock`: `none`
  - `openai_compatible`: `stable_prefix`
  - `openrouter`: `stable_prefix`
  - future Anthropic/Gemini/Doubao providers plug into the same interface without runtime changes

### Final Provider Payload Canonicalization
- Each provider integration must use one shared helper that both:
  - prepares the outbound payload
  - produces the canonical `l0_view`, `l1_view`, and `l2_view`
- `l0_view` includes provider/model/tool-choice/structured-output/runtime knobs that affect payload semantics or cache compatibility.
- `l1_view` is the stable prefix in final payload form:
  - merged `system` or `developer` field if the provider collapses them
  - top-level system field if the provider uses one
  - leading prefix messages if the provider preserves segmented prefixes
  - tool schema and provider-added fixed tool-guidance instructions
- `l2_view` contains only the remaining dynamic suffix plus `_summarization_event`.
- `PromptCacheLayoutMode` controls final-payload canonicalization:
  - `auto`: follow the provider-native payload shape
  - `single_system`: deterministically merge prefix segments before hashing and before send
  - `preserve_prefix_segments`: keep segmented prefixes only if the provider sends them as separate prefix segments

### L1 Native Prefix Caching Is Independent Of L2
- Native prefix caching is the default L1 path; L2 response caching is optional and layered on top.
- `apply_prompt_cache_hint()` is still called on the first request when the artifact has no handle if the strategy requires request annotations such as `cache_control`.
- `PromptPrefixArtifact` is the sole L1 cache value:
  - it stores the L1 hash, provider strategy, creation time, and optional opaque provider handle
  - it is refreshed after every provider observation that returns a new handle
- Collector and streaming paths must pass the real provider event list into `observe_prompt_cache_result()`. Passing an empty slice is invalid because native hit/miss or handle extraction may depend on streamed usage or metadata events.

## Public API / Type Changes
- Extend `AgentProvider` with backward-compatible default methods:
  - `prompt_cache_plan(&self, req: &AgentProviderRequest) -> Result<PromptCachePlan>`
  - `apply_prompt_cache_hint(&self, req: AgentProviderRequest, hint: &ProviderPromptCacheHint) -> AgentProviderRequest`
  - `observe_prompt_cache_result(&self, output: &AgentStepOutput, events: &[AgentProviderEvent]) -> Option<ProviderPromptCacheObservation>`
- Add new types:
  - `PromptCachePlan`
  - `PromptPrefixArtifact`
  - `ProviderPromptCacheStrategy`
  - `ProviderPromptCacheHint`
  - `ProviderPromptCacheObservation`
- Extend `ProviderCacheEvent` with:
  - `cache_source: local|provider|hybrid`
  - `provider_strategy`
  - `provider_cache_status: applied|hit|miss|unsupported`
  - `provider_handle_hash`
- Keep `RuntimeMiddleware` unchanged. `PromptCachingMiddleware` remains config/state injection only.
- Providers may add an internal helper to share outbound-payload preparation and prompt-cache planning, but the public extension surface remains the `AgentProvider` prompt-cache hooks above.
- Keep existing config fields working and add only:
  - `runtime.prompt_cache.native = auto|off|required` with default `auto`
  - `runtime.prompt_cache.layout = auto|single_system|preserve_prefix_segments` with default `auto`
- `layout=auto` rule:
  - preserve separate prefix segments only if the provider preserves them in the final payload
  - otherwise merge deterministically before send and before hash
- Native mode contract:
  - `off`: do not apply hints and do not emit provider-native status
  - `auto`: best-effort native caching, emit `unsupported` when the provider cannot participate
  - `required`: fail the run with a prompt-cache-specific runtime error if the provider cannot apply or observe native prefix caching

## Test Plan
- Preserve current passing coverage for `PC-01/02/03/07`, `PK-01/02/03/05/06`, and streaming L2-hit behavior.
- Add canonical-payload tests:
  - same final merged payload from different internal multi-system shapes yields the same L1 hash
  - `single_system` and `preserve_prefix_segments` produce different hashes only when the final sent payload differs
  - changing only the latest user message keeps L1 stable and changes L2
  - changing prompt-guided tool instructions changes L1
  - changing summarization event changes only L2
- Add provider-strategy tests:
  - native L1 reuse works with `l2=false`
  - unsupported provider emits `provider_cache_status=unsupported`
  - unsupported provider with `native=required` fails deterministically
  - native-capable provider reuses an opaque handle on the second call and reports `cache_source=hybrid`
- Add streaming/native observation tests:
  - provider events collected during streaming are passed into `observe_prompt_cache_result`
  - provider-native observation can update the L1 artifact in collector mode
- Add redaction tests:
  - trace/events never contain raw prompt text, secrets, or raw provider cache handles

## Assumptions
- This patch tightens the existing design document; it does not require a broader runtime redesign.
- Public `AgentProvider` prompt-cache hooks remain the primary extension surface.
- This design is specifically for prompt caching in the current DeepAgentRS runtime.
- In-memory local cache remains the only local store in this phase; disk/remote backends stay out of scope.
- OpenAI-compatible and OpenRouter integrations must first make final-payload hashing correct; provider-native handle support remains optional until those providers expose a concrete cache API in DeepAgentRS.
- Success criterion: cache hits and misses become explainable from provider-native payload hashes, and the runtime can adopt native cache APIs later without another architectural change.
