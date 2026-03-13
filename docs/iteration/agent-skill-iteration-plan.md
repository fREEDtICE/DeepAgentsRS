# Agent Skill Iteration Plan

- Status: Proposed
- Scope: `crates/deepagents`, `crates/deepagents-cli`, skill-related tests, and skill-related docs
- Primary decision anchor: [agent-skill-architecture-design.md](agent-skill-architecture-design.md)
- Acceptance anchors:
  - [skills acceptance index](../../acceptance_extras/skills/index.md)
  - [discovery and override](../../acceptance_extras/skills/discovery_override.md)
  - [loading and injection](../../acceptance_extras/skills/loading_injection.md)
  - [execution and isolation](../../acceptance_extras/skills/execution_isolation.md)
  - [DevX](../../acceptance_extras/skills/devx.md)

## 1. Executive Summary

The RFC now makes one architectural commitment: DeepAgentsRS should ship a single skill story based
on source-based skill packages (`SKILL.md` plus optional `tools.json`), and the unpublished
`SkillPlugin` / `AgentStep::SkillCall` / `--plugin` path should be removed before the first public
release.

This iteration plan turns that decision into a concrete sequence of deliverables. The plan is
intentionally front-loaded with simplification:

- Iteration 1 removes the user-visible legacy path so the product story becomes immediately clear.
- Iteration 2 deletes the dead protocol and runner branches so the codebase matches the story.
- Iteration 3 stabilizes determinism, cache behavior, and state boundaries for the package path.
- Iteration 4 hardens execution, validation, and isolation.
- Iteration 5 closes DevX, acceptance, and release-readiness gaps.

The result should be a skill system that is easier to explain, easier to test, and easier to
extend because every provider, runner, cache key, and CLI command follows the same package-driven
tool path.

## 2. Desired Release State

The project is ready to release this architecture when all of the following are true:

- `deepagents run` exposes only `--skills-source` for loading skills.
- Package skills are the only model-visible skill mechanism: tool specs plus the injected skills
  system block.
- `SkillPlugin`, `SkillCall`, `SkillError`, `SkillSpec`, and `DeclarativeSkillPlugin` are removed
  from the public architecture and codepath.
- `AgentStep::SkillCall` and `AgentProviderRequest.skills` are removed from the provider protocol.
- Prompt cache planning keys only derive from provider-visible request state, not dead compatibility
  fields.
- Skill discovery, tool exposure, and diagnostics are deterministic across runs and platforms.
- Child runs do not inherit skill snapshot keys or prompt-cache-private keys unintentionally.
- The acceptance matrix in the
  [skills acceptance index](../../acceptance_extras/skills/index.md) passes end to end.
- Historical docs that still describe the dual-path design are either updated or clearly marked as
  historical.

## 3. Planning Principles

- Remove dead surface before adding new capability.
- Prefer compile-time deletion over runtime hiding for unpublished interfaces.
- Land docs, help text, and tests in the same iteration as the behavior they describe.
- Keep each iteration shippable on its own; avoid one large cleanup branch.
- Use the acceptance docs as release gates, not as post-hoc documentation.

## 4. Recommended Default Decisions

These choices should be treated as iteration-planning defaults unless a later design review changes
them explicitly:

- Invalid skill source behavior: fail fast by default; keep explicit skip behavior only behind
  `--skills-skip-invalid`.
- Conflict with core tools: fail fast.
- Conflict between skill tools: last one wins, with diagnostics recorded.
- Skill output policy for the first release: keep truncation as the default package-local policy;
  do not add a second large-output mechanism just for skills in this iteration train.
- Child-state isolation: exclude `skills_metadata`, `skills_tools`, `skills_diagnostics`,
  `_prompt_cache_options`, and `_provider_cache_events` from subagent inheritance.
- Future alternative backends: if they arrive later, they should plug into the same package,
  validation, tool-spec, and runtime-control pipeline rather than reintroducing a second invocation
  protocol.

## 5. Primary Implementation Anchors

The current cleanup and hardening work concentrates around these files:

- CLI and command surface:
  - [main.rs](../../crates/deepagents-cli/src/main.rs)
- Provider protocol and adapters:
  - [protocol.rs](../../crates/deepagents/src/provider/protocol.rs)
  - [mock.rs](../../crates/deepagents/src/provider/mock.rs)
  - [llm.rs](../../crates/deepagents/src/provider/llm.rs)
  - [prompt_cache.rs](../../crates/deepagents/src/provider/prompt_cache.rs)
- Runtime orchestration:
  - [agent.rs](../../crates/deepagents/src/agent.rs)
  - [simple.rs](../../crates/deepagents/src/runtime/simple.rs)
  - [resumable_runner.rs](../../crates/deepagents/src/runtime/resumable_runner.rs)
  - [skills_middleware.rs](../../crates/deepagents/src/runtime/skills_middleware.rs)
  - [events.rs](../../crates/deepagents/src/runtime/events.rs)
- Skill loading and validation:
  - [mod.rs](../../crates/deepagents/src/skills/mod.rs)
  - [loader.rs](../../crates/deepagents/src/skills/loader.rs)
  - [validator.rs](../../crates/deepagents/src/skills/validator.rs)
  - [protocol.rs](../../crates/deepagents/src/skills/protocol.rs)
  - [declarative.rs](../../crates/deepagents/src/skills/declarative.rs)
- Existing tests that must migrate:
  - [e2e_phase1_5_runtime.rs](../../crates/deepagents-cli/tests/e2e_phase1_5_runtime.rs)
  - [skills_phase6.rs](../../crates/deepagents/tests/skills_phase6.rs)
  - [provider_prompt_guided.rs](../../crates/deepagents/tests/provider_prompt_guided.rs)
  - [provider_openai_compatible.rs](../../crates/deepagents/tests/provider_openai_compatible.rs)
  - [runner_events.rs](../../crates/deepagents/tests/runner_events.rs)

## 6. Definition Of Done

This effort is complete only when all of the following hold:

- Code:
  - no runtime constructor accepts `Vec<Arc<dyn SkillPlugin>>`
  - no provider or runner branch handles `AgentStep::SkillCall`
  - no prompt-cache implementation hashes `req.skills`
  - no CLI command or help output mentions `--plugin`
- Tests:
  - skill acceptance scenarios SD-01 through SD-04 pass
  - skill acceptance scenarios SL-01 through SL-05 pass
  - skill acceptance scenarios SEI-01 through SEI-05 pass
  - skill acceptance scenarios SDX-01 through SDX-03 pass
- Docs:
  - the RFC, iteration plan, CLI help, and examples all describe one package-only architecture
  - older docs that mention `SkillPlugin` as a supported path are updated or marked historical

## 7. Iteration Breakdown

### Iteration 0: Freeze The Migration Target

Intent:
Lock the target state before touching behavior so the removal work does not drift back into a
"temporary compatibility" project.

Scope:

- Treat the RFC as the source of truth for architecture direction.
- Publish this implementation plan next to the RFC.
- Remove stale references to the deleted legacy Phase 6 detailed plan when implementation work
  starts.
- Inventory all remaining references to `SkillPlugin`, `AgentStep::SkillCall`, `--plugin`, and
  `req.skills` across code, tests, and docs.

Deliverables:

- A committed plan document.
- A migration checklist attached to the first cleanup PR.

Exit criteria:

- There is no ambiguity inside the repo about whether `SkillPlugin` is being preserved.

### Iteration 1: Remove Legacy User-Facing Entry Points

Intent:
Make the product story clear immediately. Even before the internal cleanup is fully complete, users
and contributors should see one skills interface.

Scope:

- Remove `--plugin` from `deepagents run`.
- Remove CLI help and examples that mention the declarative plugin manifest path.
- Rewrite CLI/help text around `--skills-source`, `skill init`, `skill validate`, and `skill list`.
- Migrate any black-box CLI tests that still pass `--plugin`.

Primary files:

- [main.rs](../../crates/deepagents-cli/src/main.rs)
- [e2e_phase1_5_runtime.rs](../../crates/deepagents-cli/tests/e2e_phase1_5_runtime.rs)
- [E2E_PHASE1_5.md](../../e2e/E2E_PHASE1_5.md)
- [E2E_PHASE2.md](../../e2e/E2E_PHASE2.md)
- [TECH_DESIGN.md](../../TECH_DESIGN.md)

Concrete tasks:

- Delete the `plugin: Vec<String>` CLI argument and the corresponding loading path.
- Ensure `deepagents run --help` documents only `--skills-source`.
- Replace plugin-oriented E2E fixtures with source-directory package fixtures.
- Decide whether to keep a short-lived explicit "flag removed" parser error for internal users. If
  kept temporarily, do not document it.

Deliverables:

- A clean CLI with one supported skills path.
- Updated help snapshots and E2E fixtures.

Exit criteria:

- A new contributor can discover only the package-based skill workflow from the CLI and docs.

### Iteration 2: Remove Legacy Protocol And Runner Branches

Intent:
Delete the second invocation protocol so the architecture, type system, and runtime all agree.

Scope:

- Remove `SkillPlugin` and the declarative plugin adapter.
- Remove `AgentStep::SkillCall` from the provider protocol.
- Remove `AgentProviderRequest.skills` from request assembly and cache planning.
- Remove runtime expansion logic that maps `SkillCall -> AgentToolCall[]`.
- Remove test fixtures and mocks that manufacture `SkillCall` steps.

Primary files:

- [mod.rs](../../crates/deepagents/src/skills/mod.rs)
- [mock.rs](../../crates/deepagents/src/provider/mock.rs)
- [agent.rs](../../crates/deepagents/src/agent.rs)
- [simple.rs](../../crates/deepagents/src/runtime/simple.rs)
- [resumable_runner.rs](../../crates/deepagents/src/runtime/resumable_runner.rs)
- [events.rs](../../crates/deepagents/src/runtime/events.rs)
- [prompt_cache.rs](../../crates/deepagents/src/provider/prompt_cache.rs)

Concrete tasks:

- Delete `SkillSpec`, `SkillCall`, `SkillError`, and `SkillPlugin`.
- Remove `DeclarativeSkillPlugin` and any code path that loads JSON plugin manifests.
- Remove `skills: Vec<...>` storage from runtime builders and runners.
- Remove `AgentStep::SkillCall` handling from the runner and event classification.
- Update `MockProvider` to emit only final text, assistant text, and tool calls.
- Update tests that currently pass `skills: Vec::new()` in provider requests or expect `SkillCall`
  to remain part of the protocol.

Deliverables:

- A provider protocol with only one execution model: normal tool calling.
- A runtime that exposes only package skill tools through `tool_specs`.

Exit criteria:

- `rg` over the repo no longer finds `SkillPlugin`, `DeclarativeSkillPlugin`, or
  `AgentStep::SkillCall` in active code.

### Iteration 3: Stabilize The Package-Only Runtime Path

Intent:
Now that there is one path, make it deterministic and cache-friendly enough to rely on in release
and regression testing.

Scope:

- Make loader output ordering deterministic.
- Make model-visible skill injection deterministic.
- Remove prompt-cache dependence on dead abstract skill fields.
- Tighten state and subagent isolation for skill and cache snapshot keys.

Primary files:

- [loader.rs](../../crates/deepagents/src/skills/loader.rs)
- [skills_middleware.rs](../../crates/deepagents/src/runtime/skills_middleware.rs)
- [prompt_cache.rs](../../crates/deepagents/src/provider/prompt_cache.rs)
- [llm.rs](../../crates/deepagents/src/provider/llm.rs)
- subagent protocol and middleware files

Concrete tasks:

- Replace hash-map-order leakage with explicit sort order or `BTreeMap`-backed assembly where
  needed.
- Ensure metadata order, tool order, and override diagnostics order are stable and documented.
- Ensure `SkillsMiddleware.before_run` remains idempotent when restoring from serialized state.
- Exclude skill snapshot and prompt-cache-private keys from child-state inheritance.
- Add regression tests that snapshot:
  - injected system skills block order
  - `tool_specs` order
  - prompt-cache key stability for identical skill sources

Deliverables:

- Deterministic provider requests for identical skill inputs.
- Clear subagent state-boundary behavior for skills and cache metadata.

Exit criteria:

- The same skills sources produce the same provider-visible request shape across runs.

### Iteration 4: Harden Validation, Execution, And Isolation

Intent:
Turn the package path from "the only path" into "the safe and diagnosable path".

Scope:

- Tighten runtime input validation and failure reporting.
- Confirm deny-by-default permissions and approval composition.
- Freeze one large-output policy for skill tools.
- Remove ambiguous runtime-only tool interactions.

Primary files:

- [validator.rs](../../crates/deepagents/src/skills/validator.rs)
- [skills_middleware.rs](../../crates/deepagents/src/runtime/skills_middleware.rs)
- filesystem runtime middleware and approval/audit support files
- [skills_phase6.rs](../../crates/deepagents/tests/skills_phase6.rs)

Concrete tasks:

- Review whether runtime schema validation should reuse more of the loader/validator contract.
- Keep `allow_filesystem`, `allow_execute`, `allow_network`, `max_steps`, `timeout_ms`, and
  `max_output_chars` as the enforced boundary for package tools.
- Make the failure mode for runtime-only tools such as `task` and `compact_conversation` explicit
  and documented.
- Keep panic, timeout, permission, and schema errors explicit and non-fatal to the runner.
- Decide whether any HITL integration is part of the first-release skill gate; if yes, add E2E
  coverage now rather than later.

Deliverables:

- Hardened skill-tool execution semantics.
- Clear error taxonomy for package-skill failures.

Exit criteria:

- SEI-01 through SEI-05 pass against the package-only path.

### Iteration 5: DevX, Acceptance Closure, And Release Readiness

Intent:
Finish the operational lifecycle so the package-only system is not just correct, but practical for
contributors and users.

Scope:

- Polish `skill init`, `skill validate`, and `skill list`.
- Improve diagnostics and CI ergonomics.
- Reconcile docs and acceptance plans that still describe the legacy dual path.
- Close the full acceptance matrix.

Primary files:

- [main.rs](../../crates/deepagents-cli/src/main.rs)
- [devx.md](../../acceptance_extras/skills/devx.md)
- [index.md](../../acceptance_extras/skills/index.md)
- remaining skill-related docs under `docs/`

Concrete tasks:

- Ensure `skill init` generates a package that passes `skill validate` immediately.
- Ensure `skill validate` returns precise file and field errors and a CI-friendly exit code.
- Ensure `skill list` surfaces final skills, tools, and override diagnostics clearly.
- Update historical documents that still describe `SkillPlugin` as part of the intended release
  architecture:
  - [TECH_DESIGN.md](../../TECH_DESIGN.md)
  - [E2E_PHASE1_5.md](../../e2e/E2E_PHASE1_5.md)
  - [AGENT_STREAMING_PLAN.md](../AGENT_STREAMING_PLAN.md)
- Run and stabilize the full skill acceptance suite.

Deliverables:

- A coherent authoring and validation workflow.
- Updated docs that no longer tell two different stories.
- Release checklist evidence for the skills subsystem.

Exit criteria:

- SDX-01 through SDX-03 pass.
- The repo docs consistently describe package skills as the only supported release path.

## 8. Recommended PR Slicing

To keep review risk low, the work should land in small, narrow PRs:

1. `docs/skill-rfc-plan`: plan document plus RFC cross-links if needed.
2. `cli/remove-plugin-flag`: remove `--plugin`, migrate CLI tests and help text.
3. `protocol/remove-skillcall`: remove protocol types and runner branches.
4. `runtime/deterministic-skills`: deterministic ordering, cache cleanup, child-state filtering.
5. `skills/hardening`: validation, isolation, and failure-mode tightening.
6. `skills/devx-release`: doc reconciliation, command polish, full acceptance closure.

Each PR should update both code and tests in the same change set. Avoid landing a protocol change
without its test migrations, or a CLI/help change without the corresponding docs cleanup.

## 9. Risks And Mitigations

- Risk: removal work stalls after CLI cleanup, leaving dead internal types.
  - Mitigation: make Iteration 2 a compile-time deletion milestone, not a soft refactor milestone.
- Risk: deterministic-order fixes are postponed and prompt-cache churn remains hidden until late.
  - Mitigation: put cache-key and tool-order snapshot tests in Iteration 3, not in the release
    scramble.
- Risk: historical docs reintroduce obsolete assumptions during future work.
  - Mitigation: remove stale links to the deleted legacy Phase 6 detailed plan and keep the new
    RFC/plan pair as the only active architecture references.
- Risk: the team treats package-skill hardening as optional after legacy deletion.
  - Mitigation: use the acceptance extras as hard release gates.

## 10. Release Gate Checklist

Before calling the skill architecture release-ready, confirm all of the following:

- `cargo test` passes for `deepagents` and `deepagents-cli`.
- No active code path references `SkillPlugin`, `SkillCall`, `SkillSpec`, `SkillError`, or
  `DeclarativeSkillPlugin`.
- No public protocol or CLI help mentions `AgentStep::SkillCall`, `req.skills`, or `--plugin`.
- Package skills load, inject, execute, and fail according to the acceptance extras.
- Prompt-cache behavior is driven only by provider-visible request shape.
- All updated docs point to the package-only architecture.
