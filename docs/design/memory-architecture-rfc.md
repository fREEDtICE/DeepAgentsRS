# RFC: Target Memory Architecture for DeepAgentsRS

- Status: Draft (target-state implementation RFC)
- Scope: `crates/deepagents`, `crates/deepagents-cli`
- Product/design source: [`memory-design.md`](./memory-design.md)
- Current implementation anchors:
  - [`memory::protocol`](../../crates/deepagents/src/memory/protocol.rs)
  - [`memory::store_file`](../../crates/deepagents/src/memory/store_file.rs)
  - [`runtime::MemoryMiddleware`](../../crates/deepagents/src/runtime/memory_middleware.rs)
  - [`runtime::assembly`](../../crates/deepagents/src/runtime/assembly.rs)
  - [`state::AgentState`](../../crates/deepagents/src/state.rs)
  - [`deepagents-cli`](../../crates/deepagents-cli/src/main.rs)

## 1. Purpose and status

This RFC defines the target DeepAgentsRS memory architecture required to fully implement the design
described in [`memory-design.md`](./memory-design.md).

It is not a description of the repository's current memory subsystem. The current subsystem remains
important, but only as:

- a Phase 0 baseline
- a migration starting point
- a gap reference for implementation planning

The normative architecture in this document is the future state DeepAgentsRS should implement:

- scoped memory across thread, user, workspace, and optional system domains
- typed memory records with lifecycle and provenance
- selective retrieval instead of eager full-corpus prompt injection
- explicit memory-management flows for users and operators
- automatic memory extraction under policy control
- consolidation and auditability over time

The repository already contains useful seams that can evolve toward this architecture, but those
existing seams are not the target contract by themselves.

## 2. Goals and non-goals

### 2.1 Goals

DeepAgentsRS memory must satisfy the full goals from [`memory-design.md`](./memory-design.md):

- identity continuity across channels for one user
- isolation between different users
- deliberate workspace collaboration with shared memory
- reliable explicit memorization when the user says "remember this"
- autonomous retrieval and selective storage under policy control

The architecture should encode these goals through three first-class dimensions:

- scope
- type
- lifecycle

### 2.2 First-wave non-goals

The first implementation wave should still target the full architecture above. The following items
remain out of scope for the initial delivery only because they are deployment or packaging choices,
not because they are incompatible with the target design:

- splitting memory into a separate Rust crate before the architecture stabilizes
- requiring a remote multi-process deployment model; the first implementation may stay in-process
- building a GUI/admin console before the core runtime, storage, and CLI contracts exist

These non-goals must not be used to remove scoped memory, typed memory, selective retrieval,
explicit lifecycle management, or explainability from the target architecture.

## 3. Target architecture

### 3.1 Core principles

DeepAgentsRS memory is scoped, typed, permissioned knowledge that is selectively written,
selectively retrieved, and continuously consolidated.

The target architecture has these properties:

- every durable memory belongs to exactly one scope
- every durable memory has an explicit type and lifecycle state
- permission checks happen before retrieval and before write commitment
- the model receives a compact memory package, not the full raw corpus
- explicit user memory commands are first-class runtime operations
- automatic writes are policy-gated and auditable
- stale or conflicting memories are revised through lifecycle transitions, not silent overwrite

### 3.2 Scope model

DeepAgentsRS supports these durable scopes:

1. `Thread`

   - channel/thread-local context
   - visible only inside that thread

2. `User`

   - private memory shared across that user's channels
   - default scope for explicit personal memorization

3. `Workspace`

   - shared memory available to workspace members only
   - used for shared facts, decisions, conventions, tasks, and summaries

4. `System` (optional)

   - agent-owned heuristics, cache metadata, or operational summaries
   - not surfaced as user memory

Configuration layering terms such as `global` or `workspace` in config resolution are not a
substitute for runtime memory scopes. Runtime scope is modeled explicitly in memory data and access
policy.

### 3.3 Type model

Each durable memory item carries one primary `MemoryType`:

- `Profile`
- `Episodic`
- `Semantic`
- `Procedural`
- `Pinned`

The runtime also uses short-lived working memory for the active turn, but working memory is not a
durable stored type unless promoted by the write pipeline.

### 3.4 Lifecycle model

Every durable memory item must support lifecycle semantics:

- creation with provenance
- retrieval with ranking and policy checks
- correction through supersession
- temporary expiration where appropriate
- explicit delete or inactive status
- consolidation into summaries

Capacity eviction is an operational storage concern. It is not a substitute for delete, decay,
expiration, supersession, or provenance.

### 3.5 High-level architecture layers

The target memory architecture has these layers:

| Layer | Responsibility | Primary output |
| --- | --- | --- |
| Identity and tenancy | Resolve users, channels, threads, workspaces, memberships | scoped runtime context |
| Durable model and repositories | Persist typed memory, links, summaries, audit data | queryable typed records |
| Retrieval | classify intent, decompose queries, rank memory, assemble prompt package | `MemoryContextPack` |
| Write pipeline | explicit writes, automatic extraction, policy gates, dedupe, supersession | committed mutations |
| Consolidation | summarize, merge, stale-mark, produce memory cards | summary memory and links |
| Runtime integration | inject compact memory context and schedule post-response write evaluation | provider-visible context and write tasks |
| User/operator surfaces | remember/list/get/edit/delete/pin/unpin/explain/settings | controlled memory management |

## 4. DeepAgentsRS module design

The implementation should stay inside `crates/deepagents` for the first wave, using focused
submodules rather than a new crate split.

### 4.1 `memory::model`

Purpose:

- define the typed memory domain model
- encode scope, type, lifecycle, provenance, and identifiers in Rust types

Primary types:

- `UserId`, `ChannelAccountId`, `ThreadId`, `WorkspaceId`, `MessageId`, `MemoryId`
- `MemoryScope`
- `MemoryType`
- `MemoryStatus`
- `PrivacyLevel`
- `MemorySourceKind`
- `MemoryItem`
- `MemoryLink`
- `MemorySummary`

Required `MemoryItem` fields:

- `memory_id`
- `scope`
- `memory_type`
- `title`
- `content`
- `source`
- `confidence`
- `salience`
- `privacy_level`
- `pinned`
- `status`
- `created_at`
- `updated_at`
- `valid_from`
- `valid_to`
- `supersedes`
- `tags`

### 4.2 `memory::identity`

Purpose:

- model the identity graph required by scoped retrieval and writes
- resolve runtime context from channel and thread inputs

Primary domain types:

- `User`
- `ChannelAccount`
- `Thread`
- `Workspace`
- `WorkspaceMembership`

Primary trait:

- `IdentityResolver`

Required responsibilities:

- resolve channel identity to canonical `UserId`
- resolve or create durable `ThreadId`
- detect workspace context
- verify workspace membership for workspace scope access

### 4.3 `memory::store`

Purpose:

- split operational persistence from semantic retrieval indexing
- keep typed storage interfaces separate from runtime prompt assembly

Primary traits:

- `MemoryRepository`
- `MemoryLinkRepository`
- `MemorySummaryRepository`
- `MemoryAuditRepository`
- `EmbeddingIndex`

Required `MemoryRepository` responsibilities:

- create and update typed memory items
- read by `MemoryId`
- list and query by scope/status/type/tags
- support lifecycle transitions such as pin, unpin, delete, supersede, expire
- preserve typed timestamps and provenance references

The current `MemoryStore` and file backend can remain as a Phase 0 compatibility layer, but they do
not define the target repository contract.

### 4.4 `memory::retrieval`

Purpose:

- perform layered retrieval for each turn
- convert typed memory into a compact provider-facing memory package

Primary trait:

- `MemoryRetrievalService`

Primary inputs:

- resolved identity context
- current thread context
- current user message
- retrieval policy

Primary outputs:

- `MemoryContextPack`
- retrieval diagnostics and warnings

Required behaviors:

- classify message intent and topic
- decompose into multiple retrieval queries
- rank results using semantic similarity, recency, salience, confidence, pin boost, and scope
  priority
- prefer summaries when detail volume is too large
- return a structured prompt package instead of injecting full raw memory

`MemoryContextPack` should include compact sections such as:

- `user_profile`
- `user_preferences`
- `active_goals`
- `relevant_episodic`
- `workspace_context`
- `memory_warnings`

### 4.5 `memory::write`

Purpose:

- turn explicit user instructions and automatic candidates into committed memory mutations

Primary trait:

- `MemoryWriteService`

Primary types:

- `MemoryWriteRequest`
- `MemoryCandidate`
- `MemoryMutation`
- `MemoryWriteDecision`

Required behaviors:

- explicit memorization flow for "remember X"
- scope inference with user-private default and workspace override when clearly shared
- type classification
- dedupe and merge
- supersession on contradiction
- user confirmation for explicit writes
- re-index and summary refresh after committed mutations

### 4.6 `memory::policy`

Purpose:

- centralize privacy, retention, sensitivity, and access-control rules

Primary trait:

- `MemoryPolicyService`

Required behaviors:

- authorize read and write by scope and membership
- enforce sensitive-data restrictions
- apply auto-memory settings
- validate allowed scope transitions
- supply explainability data for user-facing responses

### 4.7 `memory::consolidation`

Purpose:

- reduce noise and improve retrieval quality over time

Primary trait:

- `MemoryConsolidationService`

Required behaviors:

- summarize related episodic records
- promote repeated patterns into semantic or procedural memory
- merge duplicates
- mark stale memories
- emit summary cards for preferences, goals, and workspace state

### 4.8 `memory::audit`

Purpose:

- preserve provenance and user-visible explanation records

Primary responsibilities:

- track why a memory exists
- link memories to source messages and superseded items
- support "what do you remember, why, and from where?" queries

### 4.9 `runtime::memory_context_middleware`

Purpose:

- replace eager raw file injection as the normative runtime architecture
- bridge the runtime pipeline with identity resolution, retrieval, and post-response write handling

Required behaviors:

1. resolve runtime identity and workspace context
2. invoke retrieval and build a compact memory context pack
3. inject the structured package into the provider-visible prompt
4. preserve diagnostics in runtime state
5. after the response, evaluate explicit and automatic write candidates
6. schedule writes or confirmations through the write service

The current `runtime::MemoryMiddleware` remains a migration-era component only.

### 4.10 CLI and operator surfaces

The target user/operator surface must support:

- `remember`
- `list`
- `get`
- `edit`
- `delete`
- `pin`
- `unpin`
- scope selection
- provenance/explain output
- auto-memory settings

The CLI can expose these as `deepagents memory ...` subcommands, but the RFC should define the
behavioral contract first and not treat today's narrow CLI surface as sufficient.

## 5. Core flows

### 5.1 On message receive

```text
1. Resolve channel identity to user_id.
2. Resolve thread_id.
3. Detect workspace context and membership.
4. Classify intent and retrieval intent.
5. Retrieve memory in layers:
   working -> thread -> pinned -> user -> workspace -> summaries
6. Build a compact MemoryContextPack.
7. Inject the pack into the provider-visible prompt.
8. Generate the reply.
9. Evaluate explicit and automatic write candidates.
10. Apply policy gates, dedupe, and supersession logic.
11. Commit approved mutations and refresh derived summaries/indexes.
```

### 5.2 On explicit "remember this"

```text
1. Parse the requested content.
2. Infer the target scope:
   default to user scope
   use workspace scope only when the request is clearly shared/team-oriented
3. Classify the memory type.
4. Apply policy and sensitivity checks.
5. Store as pinned memory.
6. Return confirmation that states what was remembered and where.
7. Allow later edit, unpin, delete, or scope-correcting follow-up.
```

Manual file editing or ad hoc `put` operations are not the target explicit memorization protocol.

### 5.3 On contradiction or correction

```text
1. Find conflicting active memories in the same scope.
2. Prefer newer explicit statements when conflict exists.
3. Mark the older memory as superseded or inactive.
4. Update links and audit records.
5. Recompute affected summaries and retrieval indexes.
```

### 5.4 On forgetting or deletion

```text
1. Resolve the target memory item.
2. Validate caller permission for that scope.
3. Apply the requested lifecycle transition:
   unpin
   expire
   inactive/delete
4. Preserve audit and provenance records unless retention policy requires purge.
5. Update retrieval summaries and indexes.
```

### 5.5 On consolidation

```text
1. Group related episodic memories by entity and topic.
2. Identify repeated facts and duplicate records.
3. Produce semantic/procedural summaries.
4. Mark stale or redundant records appropriately.
5. Persist summaries, links, and audit references.
```

## 6. Data model and public interfaces

### 6.1 Durable data model

The target relational/logical model should include:

- `users`
- `channel_accounts`
- `threads`
- `workspaces`
- `workspace_memberships`
- `messages`
- `memory_items`
- `memory_links`
- `memory_summaries`
- `memory_access_policies`
- `memory_audit_logs`

Required logical indexes include:

- by `(scope_type, scope_id, status)`
- by `memory_type`
- by `pinned`
- by `updated_at`
- by `tags`
- vector/embedding index for semantic retrieval

### 6.2 Target Rust interface set

The RFC should treat the following as target interfaces even if they are introduced incrementally:

- `MemoryScope`
  - supports `Thread`, `User`, `Workspace`, and optional `System`
- `MemoryItem`
  - typed durable record with scope, type, provenance, confidence, salience, pin, lifecycle, and
    supersession data
- `MemoryRepository`
  - typed CRUD, scoped listing, lifecycle transitions, and supersession management
- `EmbeddingIndex`
  - semantic retrieval over scoped memories and summaries
- `MemoryRetrievalService`
  - input: resolved identity/context plus the current message
  - output: `MemoryContextPack`
- `MemoryWriteService`
  - explicit memorization plus automatic candidate handling
- `MemoryPolicyService`
  - privacy, sensitivity, scope legality, retention, explainability
- `MemoryAuditRepository`
  - provenance and user-visible explanation records

### 6.3 Prompt-facing contract

The provider-visible memory package should be compact and structured. The runtime must not inject
the entire durable memory corpus by default.

The prompt package should be shaped around retrieval intent, for example:

```json
{
  "user_profile": [],
  "user_preferences": [],
  "active_goals": [],
  "relevant_episodic": [],
  "workspace_context": [],
  "memory_warnings": []
}
```

This package may be rendered as text for the provider, but the structure must be derived from typed
retrieval output, not from raw filesystem concatenation.

## 7. Migration plan

### Phase 0: current baseline

Preserve the current memory subsystem as the starting point:

- `memory::protocol` provides `MemoryEntry`, `MemoryQuery`, `MemoryDiagnostics`, and `MemoryStore`
- `memory::store_file` provides local JSON-backed storage and `AGENTS.md` export
- `runtime::MemoryMiddleware` performs eager filesystem memory loading and prompt injection
- `deepagents-cli` exposes `memory put`, `memory query`, and `memory compact`

Phase 0 remains supported while the target architecture is introduced incrementally.

### Phase 1: typed domain model and identity-aware repositories

- add `memory::model` and `memory::identity`
- introduce typed identifiers, scope/type/status/provenance model, and identity resolution
- introduce repository traits without removing the Phase 0 file backend

### Phase 2: selective retrieval runtime

- add `memory::retrieval`
- introduce ranked retrieval and `MemoryContextPack`
- add `runtime::memory_context_middleware`
- de-emphasize eager full-corpus `AGENTS.md` injection as the primary runtime model

### Phase 3: explicit memory-management surface

- add user/operator list/get/edit/delete/pin/unpin/scope/explain/settings support
- keep CLI and runtime behavior aligned with typed repository semantics

### Phase 4: automatic write pipeline and policy gates

- add `memory::write` and `memory::policy`
- implement explicit write confirmation, automatic candidate handling, sensitivity filters,
  dedupe, and supersession

### Phase 5: consolidation and audit completeness

- add `memory::consolidation` and `memory::audit`
- implement summaries, stale marking, summary cards, and full provenance/explainability

### Migration constraints

During the migration, new work must preserve these useful existing guarantees unless the change is
explicitly replaced by a stronger typed guarantee:

- deterministic runtime middleware ordering
- bounded prompt growth
- root-bound path safety where filesystem-backed sources remain in use
- private runtime state for non-public memory artifacts
- subagent isolation for memory that should not be inherited automatically

## 8. Current-system gap analysis

This section compares the target architecture above against the repository as it exists today.

### 8.1 Gap summary

| Target capability | Current seam | Status today | Gap |
| --- | --- | --- | --- |
| Canonical identity graph | `runtime` thread handling only | Not implemented | No `User` / `ChannelAccount` / `WorkspaceMembership` model or resolver exists |
| Durable scoped memory | `MemoryEntry` in `memory::protocol` | Not implemented | Entries have no thread/user/workspace/system scope |
| Durable typed memory | `MemoryEntry` in `memory::protocol` | Not implemented | Entries have no profile/episodic/semantic/procedural/pinned type |
| Lifecycle and provenance | file store + eviction | Not implemented | There is no typed delete/edit/supersede/expire/provenance model |
| Selective retrieval | `runtime::MemoryMiddleware` | Not implemented | Runtime injects eager filesystem text instead of ranked layered retrieval |
| Explicit "remember this" runtime flow | prompt guidelines + manual tools | Not implemented | No dedicated parse/scope/classify/confirm memory-write path exists |
| Automatic extraction | none | Not implemented | No candidate detection, policy gate, or automatic write pipeline exists |
| Consolidation and summaries | none | Not implemented | No summary generation, semantic promotion, or stale marking exists |
| Scoped permission model | none | Not implemented | No user/private/workspace access-control layer exists |
| User memory controls | narrow CLI | Not implemented | No list/get/edit/delete/pin/unpin/explain/settings contract exists |
| Audit and explainability | none | Not implemented | System cannot explain what it remembers, why, and from where |
| Hybrid retrieval storage | local JSON + text injection | Not implemented | No typed repository plus semantic retrieval index exists |

### 8.2 `memory::protocol` vs target model

Current `MemoryEntry` is not the target memory model.

Missing from the current entry contract:

- scope
- memory type
- provenance
- confidence
- salience
- privacy level
- lifecycle status
- supersession linkage
- typed access policy

Current `MemoryQuery` is not the target retrieval contract. It only supports `prefix`, `tag`, and
`limit`, so it cannot represent scoped semantic retrieval, ranking, intent-aware query
decomposition, or summary preference.

### 8.3 `memory::store_file` vs target storage layer

The current file backend is useful as a Phase 0 local store, but it is a transitional backend only.

What it provides today:

- local durability
- atomic file writes
- coarse query by prefix/tag
- bounded eviction
- `AGENTS.md` projection

What it does not provide:

- typed lifecycle transitions
- scoped access control
- provenance and audit logging
- semantic retrieval index
- separate repositories for items, links, summaries, and audit records

Eviction must be documented strictly as bounded-storage behavior. It is not forgetting, correction,
supersession, or provenance.

### 8.4 `runtime::MemoryMiddleware` vs target runtime

The current middleware is an interim mechanism, not the target runtime architecture.

What it does today:

- loads configured `AGENTS.md` sources
- enforces path and size rules
- stores raw contents in `AgentState.private`
- injects a single `<agent_memory>` system block

What it does not do:

- resolve channel identity to canonical user identity
- detect workspace membership and scope access
- classify retrieval intent
- run layered retrieval
- produce a structured `MemoryContextPack`
- evaluate post-response write candidates
- support explicit memorization as a typed runtime flow

Manual file editing or generic `put` operations should be described as operator workarounds only,
not as partial implementation of the target explicit memorization protocol.

### 8.5 CLI surface vs target user controls

Current CLI coverage is materially below the target contract.

What exists today:

- `deepagents memory put`
- `deepagents memory query`
- `deepagents memory compact`

What remains missing:

- `list`
- `get`
- `edit`
- `delete`
- `pin`
- `unpin`
- scope selection
- provenance/explain output
- auto-memory settings

User controls are a first-class missing capability. The target system must support:

- see remembered items
- pin or unpin memory
- edit memory
- delete or forget memory
- choose whether auto-memory is enabled
- mark memory as personal or workspace-shared
- understand why a memory exists and where it came from

## 9. Verification plan

Implementation should not be considered complete until the following acceptance scenarios are
covered with unit, integration, and CLI/runtime end-to-end tests as appropriate.

### 9.1 Identity and scope

- one user can retrieve the same user-scoped memory across multiple channels
- one thread's thread-scoped memory does not leak into another thread
- workspace-scoped memory is visible only to workspace members
- personal memory does not leak into workspace-shared memory unless explicitly written there

### 9.2 Explicit memory management

- "remember this" defaults to user scope when the request is personal
- "remember this" uses workspace scope when the request is explicitly shared/team-oriented
- the system confirms what was remembered and where
- users can list, get, edit, delete, pin, and unpin memories
- users can request an explanation of why a memory exists and where it came from

### 9.3 Automatic writes and lifecycle

- durable high-signal facts can be stored automatically when policy allows
- sensitive or low-signal content is rejected by policy
- contradictory memories create supersession rather than silent overwrite
- temporary memories expire without corrupting audit state
- delete/inactive flows remain distinct from capacity eviction

### 9.4 Retrieval and consolidation

- retrieval returns a compact `MemoryContextPack` rather than the raw corpus
- personal requests prioritize user memory correctly
- workspace/project requests prioritize workspace memory correctly
- summaries replace noisy episodic detail when memory volume grows
- consolidation produces semantic/procedural summaries from repeated episodes

### 9.5 Migration safety

- Phase 0 compatibility remains available while new modules are introduced
- middleware ordering remains deterministic
- private runtime state and subagent isolation remain protected during migration
- new retrieval and write flows can coexist with legacy file-backed storage during rollout
