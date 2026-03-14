# RFC: Memory Architecture for DeepAgentsRS

- Status: Draft (implementation RFC for target-state memory)
- Scope: `crates/deepagents`, `crates/deepagents-cli`, future storage adapters, future background workers
- Design source: [`memory-design.md`](./memory-design.md)
- Current baseline anchors:
  - [`memory::protocol`](../../crates/deepagents/src/memory/protocol.rs)
  - [`memory::store_file`](../../crates/deepagents/src/memory/store_file.rs)
  - [`runtime::MemoryMiddleware`](../../crates/deepagents/src/runtime/memory_middleware.rs)
  - [`state::AgentState`](../../crates/deepagents/src/state.rs)
  - [`deepagents-cli`](../../crates/deepagents-cli/src/main.rs)
  - [`memory_phase7` tests](../../crates/deepagents/tests/memory_phase7.rs)
  - [`e2e_memory` tests](../../crates/deepagents-cli/tests/e2e_memory.rs)
- Planned implementation seams:
  - `memory::identity`
  - `memory::schema`
  - `memory::access`
  - `memory::retrieval`
  - `memory::write_service`
  - `memory::consolidation`
  - `memory::audit`
  - `runtime::MemoryMiddleware` as retrieval-pack injector rather than raw file loader

## Purpose

[`memory-design.md`](./memory-design.md) is the product and architecture target. It explains what
DeepAgentsRS memory should eventually do.

This RFC exists for a different reason: it translates that design into an implementation contract.
It should be detailed enough that engineering work can be planned and reviewed against it without
needing to infer missing data models, runtime flows, storage seams, or test scope from prose.

The current repository does not yet implement the target design. It still ships a root-scoped,
file-backed baseline. This RFC therefore has two jobs:

- define the full target implementation required to realize [`memory-design.md`](./memory-design.md)
- define the migration path from the current baseline to that target without losing current privacy,
  operability, or determinism guarantees

## Relationship To `memory-design.md`

`memory-design.md` remains the target-state design. This RFC is the implementation-facing contract
for reaching that target.

Read the two documents as:

- [`memory-design.md`](./memory-design.md): what the system should become
- `memory-architecture-rfc.md`: how the repository should be structured, migrated, verified, and
  staged to fully implement that design

The current RFC that only described the shipped file-backed baseline was not sufficient for that
goal because it lacked:

- a target durable schema
- a target identity and access model
- retrieval and write pipeline contracts
- migration rules from `memory_store.json` plus `AGENTS.md`
- implementation seams inside the repo
- CLI and runtime surfaces for explicit memory lifecycle operations
- a target verification matrix for full scoped memory

This RFC fills those gaps.

## Current Baseline

The current repository already implements a useful memory baseline:

- file-backed durable storage through `memory_store.json`
- generated `AGENTS.md` projection
- deterministic prompt injection of file-backed memory
- CLI maintenance via `put`, `get`, `delete`, `query`, and `compact`
- bounded eviction and prompt-size controls
- private runtime state and subagent filtering

That baseline remains the migration starting point. It is not the target architecture.

| Area | Current baseline | Target architecture |
| --- | --- | --- |
| Identity | workspace root only | canonical `User` / `ChannelAccount` / `Thread` / `Workspace` graph |
| Durable scope | none in entry schema | `thread`, `user`, `workspace`, optional `system` |
| Memory types | untyped `key/value/tags` | `profile`, `episodic`, `semantic`, `procedural`, `pinned` |
| Retrieval | eager `AGENTS.md` prompt injection | layered scoped retrieval plus compact memory pack |
| Writes | explicit CLI/file edits only | explicit memory tool plus controlled auto-extraction |
| Lifecycle | hard delete plus eviction | edit, pin/unpin, supersession, expiration, soft delete |
| Storage | local JSON plus markdown | operational store plus vector retrieval plus optional blobs/cache |
| Access control | root-bound filesystem | user/workspace membership and scope-aware policy |

## Goals

The implementation defined by this RFC must deliver all of the following.

### Product and behavior goals

- one canonical user identity across channels
- private user memory across channels
- thread-local memory for channel-specific context
- shared workspace memory with membership checks
- explicit and reliable "remember this" handling
- selective retrieval rather than full prompt dumping
- durable correction, supersession, forgetting, and inspection flows
- controlled automatic memory creation for high-signal durable information
- periodic consolidation from noisy episodic memories into compact semantic memory

### Engineering goals

- small, testable module seams inside the Rust codebase
- deterministic behavior under explicit policies
- auditable provenance for every durable memory item
- compatibility with the current baseline during migration
- black-box E2E coverage for all release-facing behaviors

## Non-goals

This RFC does not require:

- arbitrary unrestricted model autonomy for memory writes
- remote cloud services as the only valid deployment model
- vector search as the only retrieval mode
- leaking workspace memory into user-private reads
- retaining the current `AGENTS.md`-only runtime model as the long-term contract

## Central Decisions

- The target memory system is scoped, typed, permissioned knowledge, not a generic text dump.
- Canonical identity is required. Memory scopes are meaningless without authoritative
  `User` / `Thread` / `Workspace` resolution.
- `thread`, `user`, and `workspace` are the three required durable scopes. `system` is optional and
  internal.
- Durable memory types are required in the stored schema, not only inferred at retrieval time.
- The source of truth must move to a structured operational store. `AGENTS.md` becomes a derived or
  compatibility artifact, not the primary contract.
- Retrieval must be layered and ranked. The runtime must stop treating memory as one monolithic
  prefix block.
- Explicit memory writes are first-class product behavior and need a dedicated structured path.
- Automatic memory extraction is allowed only behind policy, confidence, and audit gates.
- Forgetting must default to soft lifecycle transitions and supersession, not blind hard delete.
- Full implementation is staged. The current file-backed baseline remains supported until the target
  storage and runtime surfaces are proven.

## Target Architecture

### Logical layers

| Layer | Responsibility | Planned seam |
| --- | --- | --- |
| Identity | Canonical users, channel mappings, threads, workspaces, memberships | `memory::identity` |
| Durable schema | Typed memory records, links, summaries, lifecycle state | `memory::schema` |
| Access control | Scope-aware reads/writes, privacy policy, sensitive-data rules | `memory::access` |
| Write pipeline | Explicit remember flows, auto-extraction, dedupe, supersession | `memory::write_service` |
| Retrieval | Query decomposition, ranking, scoped candidate assembly, memory pack shaping | `memory::retrieval` |
| Consolidation | Summaries, semantic distillation, stale marking, re-embedding | `memory::consolidation` |
| Audit | Provenance, lifecycle changes, retrieval and write reasons | `memory::audit` |
| Runtime integration | Inject compact memory packs, expose explicit memory tools, preserve privacy | `runtime::MemoryMiddleware` |
| CLI and operators | Inspect, edit, pin, forget, rebuild, audit, tune policies | `deepagents-cli` |

### Deployment shape

This RFC defines logical seams, not mandatory microservices. The first implementation may keep
these seams inside the existing Rust workspace, with local adapters where necessary. The seams still
matter because they are the unit of testing, migration, and later scaling.

## Canonical Identity Model

### Required entities

- `User`
- `ChannelAccount`
- `Thread`
- `Workspace`
- `WorkspaceMembership`
- `Message`
- `MemoryItem`
- `MemoryLink`
- `MemorySummary`
- `MemoryAuditLog`

### Required identity rules

- every inbound channel identity resolves to one canonical `user_id`
- every message resolves to a canonical `thread_id`
- every thread may optionally resolve to a `workspace_id`
- a user may belong to multiple workspaces
- workspace membership is checked at read and write time for workspace memory
- cross-user reads are denied by default

### Repository implementation notes

- the first implementation may keep identity resolution as traits plus local adapters rather than
  hardcoding one storage engine
- current local-root runs need a compatibility identity mode so the repo can operate without a full
  remote identity service during migration
- compatibility mode must still produce stable internal IDs so scoped memory logic can be exercised
  in tests

## Durable Memory Model

### Required scopes

- `thread`
- `user`
- `workspace`
- optional `system`

### Required durable types

- `profile`
- `episodic`
- `semantic`
- `procedural`
- `pinned`

Working memory is runtime-only. It is not a durable `MemoryItem` unless promoted.

### Required lifecycle states

- `active`
- `superseded`
- `inactive`
- `deleted`
- `expired`

### Required source kinds

- `explicit_user_request`
- `extracted_from_message`
- `inferred`
- `workspace_event`
- `system_imported`
- `consolidated_summary`

### Target `MemoryItem` record

The durable record must expand beyond the current `key/value/tags` model. A target record shape is:

```json
{
  "memory_id": "mem_123",
  "scope_type": "user",
  "scope_id": "user_456",
  "memory_type": "semantic",
  "title": "User prefers concise morning summaries",
  "content": "The user prefers concise summaries in the morning and detailed responses in the evening.",
  "source": {
    "kind": "inferred",
    "message_ids": ["msg_1", "msg_9", "msg_15"]
  },
  "author": "agent",
  "confidence": 0.86,
  "salience": 0.72,
  "privacy_level": "private",
  "pinned": false,
  "created_at": "2026-03-13T10:00:00Z",
  "updated_at": "2026-03-13T10:00:00Z",
  "valid_from": "2026-03-13T10:00:00Z",
  "valid_to": null,
  "supersedes": null,
  "embedding_ref": "emb_123",
  "tags": ["preference", "response-style"],
  "status": "active"
}
```

### Required fields

- `memory_id`
- `scope_type`
- `scope_id`
- `memory_type`
- `title`
- `content`
- `source.kind`
- `source.message_ids` or equivalent provenance references
- `author`
- `confidence`
- `salience`
- `privacy_level`
- `pinned`
- `created_at`
- `updated_at`
- `valid_from`
- `valid_to`
- `supersedes`
- `tags`
- `status`

### Field semantics

- `scope_type` and `scope_id` define ownership and access-control lookup
- `memory_type` drives retrieval weighting and lifecycle policy
- `confidence` captures certainty; it is not a permission bypass
- `salience` captures long-term importance for retrieval
- `pinned` is a product-level retention and ranking signal
- `supersedes` links new memory to corrected or outdated memory
- `status` tracks lifecycle without immediately destroying auditability
- `embedding_ref` may point to a vector record rather than inlining the vector in the operational row

## Storage Architecture

### Required storage layers

- operational store for structured truth
- vector index for semantic retrieval
- optional blob/document store for large raw artifacts
- scope-aware cache for hot retrieval packs and summary nodes

### Operational store requirements

The source of truth must support:

- row-level or equivalent scope filtering
- durable versioning and lifecycle state
- links between memories and messages
- audit-friendly history
- structured queries by scope, type, status, tags, timestamps, and pinned state

### Vector layer requirements

The semantic layer must support:

- separate namespaces or hard partitions for `user` and `workspace` memory
- semantic similarity retrieval over memory content and summaries
- re-embedding on content change or consolidation
- traceability from vector hits back to canonical `memory_id`

### Blob layer requirements

Optional but supported for:

- long raw notes
- raw conversation snapshots
- attachments that are referenced by memory but not embedded directly in the prompt

### Cache requirements

Cache keys must include:

- scope identity
- retrieval intent or query class
- version or watermark of underlying memory state
- privacy boundary so one scope never receives another scope's cached pack

### Repository implementation note

The existing file-backed store remains valid only as:

- a compatibility backend
- a local-development backend
- a migration source

It is not the target source of truth.

## Relational Data Model

### Required tables

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

### Required indexes

- `(scope_type, scope_id, status)`
- `(scope_type, scope_id, memory_type, status)`
- `(scope_type, scope_id, pinned, status)`
- `(scope_type, scope_id, updated_at)`
- `(scope_type, scope_id, valid_to)`
- `tags`
- `supersedes`
- vector index keyed by `memory_id`

### `memory_links` responsibilities

`memory_links` must represent:

- message provenance
- supersession chains
- summary source sets
- cross-entity relationships if later needed

## Access Control And Privacy

### Required read rules

- `thread` memory: only in that canonical thread context
- `user` memory: only for the resolved canonical user
- `workspace` memory: only when the current user is a member of that workspace
- `system` memory: internal only unless explicitly surfaced

### Required write rules

- explicit remember defaults to `user` scope unless the user clearly intends workspace sharing
- workspace writes require a resolved workspace context and allowed membership
- thread memory does not automatically promote to user or workspace memory
- sensitive data must be blocked or require stronger consent rules

### Required user controls

Users must be able to:

- inspect remembered items
- edit memory
- pin and unpin memory
- delete or forget memory
- correct outdated memory
- control auto-memory behavior
- distinguish personal vs workspace-shared memory

### Privacy guarantees during migration

Current guarantees must remain true while the target system is introduced:

- raw memory contents stay out of serialized public run state unless explicitly returned
- child runs do not inherit private parent memory wholesale
- scope checks happen before retrieval-pack assembly, not only after retrieval

## Retrieval Architecture

### Retrieval inputs

Every retrieval request must have:

- resolved `user_id`
- resolved `thread_id`
- optional `workspace_id`
- current message content
- current run context and active task metadata
- access-control context

### Required retrieval order

1. working memory
2. recent thread memory
3. pinned explicit memory
4. relevant user memory
5. relevant workspace memory
6. summaries if detailed items would overflow the prompt budget

### Required retrieval phases

1. Intent classification
2. Scope resolution
3. Query decomposition
4. Candidate gathering
5. Access filtering
6. Scoring and ranking
7. Summary substitution if needed
8. Memory pack shaping

### Intent classes

At minimum, the retriever must classify whether the message is:

- personal
- workspace/project
- mixed
- factual recall
- planning/task-related
- preference-sensitive
- explicit memory command

### Query decomposition

The retriever must support more than one query per turn. At minimum:

- direct semantic query from the user message
- hidden preference query
- hidden active-goals query
- hidden workspace-context query when in a workspace thread

### Ranking contract

The retriever must combine at least:

- semantic similarity
- recency
- salience
- pinned bonus
- scope priority
- confidence

The weighting may be tuned, but the implementation must expose a deterministic ranking trace so
retrieval decisions are explainable in tests and audit logs.

### Required output

The runtime must stop injecting arbitrary raw corpus text. Retrieval should instead produce a
compact pack shaped roughly like:

```json
{
  "user_profile": [...],
  "user_preferences": [...],
  "active_goals": [...],
  "relevant_episodic": [...],
  "workspace_context": [...],
  "memory_warnings": [...]
}
```

The exact field names may evolve, but the pack must remain:

- bounded
- typed
- deterministic
- explainable
- easy to audit

## Write Architecture

### Write sources

The write pipeline must handle:

- explicit user instructions
- extracted durable facts
- inferred repeated preferences
- workspace decisions
- consolidation outputs

### Explicit remember flow

When the user says "remember X", the system must:

1. parse the target content
2. infer scope, defaulting to `user`
3. classify memory type
4. apply privacy and sensitive-data policy
5. deduplicate or merge if applicable
6. write a pinned memory item
7. confirm what was stored and where

### Automatic extraction flow

Automatic writes are permitted only through the write service and must include:

1. candidate detection
2. scope classification
3. type classification
4. policy and privacy filtering
5. deduplication and merge
6. write with provenance
7. re-embedding and summary update scheduling

### Auto-memory thresholds

At minimum:

- explicit request: store unless policy blocks it
- strong extracted fact: store only above high confidence
- inferred preference: require repeated evidence
- sensitive content: explicit consent or deny

### Deduplication and supersession rules

The write path must:

- detect near-duplicates within the same scope and type
- merge when the new candidate is additive
- supersede when the new candidate contradicts an active fact
- preserve provenance for both the old and new records

### Required structured write seam

The runtime must not rely on generic file editing for memory writes. It needs a dedicated memory
tool or equivalent structured call path so write policy, audit, and user confirmation are enforceable.

## Correction, Forgetting, And Lifecycle

### Required lifecycle operations

- get
- query/list
- edit
- pin
- unpin
- delete/forget
- correct
- inspect provenance

### Delete semantics

The target model does not hard-delete by default. It must:

- mark memory `deleted` or `inactive`
- preserve auditability and provenance
- apply actual retention deletion only under explicit policy

### Contradiction handling

When a newer explicit fact conflicts with an older active fact, the system must:

1. find conflicting active memories in the same scope
2. prefer newer explicit statements over weaker inferred ones
3. mark the older memory as superseded
4. update summary memory and embeddings

### Expiration and decay

The target system must support:

- expiration for explicitly temporary memory
- lower ranking for stale low-value episodic memory
- periodic stale marking during consolidation

## Consolidation Architecture

### Required consolidation responsibilities

- cluster related episodic memories
- create semantic summaries
- merge duplicates
- mark stale memories
- rebuild derived summary cards

### Required outputs

The system must be able to produce scope-aware summary memory such as:

- user preferences summary
- active goals summary
- workspace project summary

### Execution model

Consolidation may run:

- asynchronously as a background job
- on-demand via operator command
- opportunistically after enough writes

The implementation seam must be explicit so the repository can test it independently of the main
runtime loop.

## Runtime Integration

### `before_run` target flow

On each user message:

1. resolve identity and workspace context
2. classify intent
3. retrieve a ranked memory pack
4. store the pack in private runtime state
5. inject a deterministic provider-visible memory section
6. keep retrieval diagnostics in public extra state
7. after response generation, evaluate write candidates if auto-memory is enabled

### Provider-visible contract

The runtime may still use a system message, but the injected content should be a shaped memory pack
rather than raw `AGENTS.md` text. The provider-visible contract must remain deterministic for cache
behavior and testability.

### Private state contract

The runtime must maintain private state for:

- raw retrieval candidates if needed
- write candidates pending confirmation
- non-public provenance references

Public `RunOutput.state` should contain only bounded diagnostics and explicitly safe summaries.

### Subagent rules

Child runs must not inherit parent private memory wholesale. If a subagent needs memory context, it
must receive a bounded derived pack rather than direct access to the parent's entire private memory.

## CLI And Operator Surfaces

### Required runtime surfaces

- `deepagents run` must support the target retrieval runtime
- explicit memory writes must use a structured tool or equivalent operator path
- operator flags for memory budget and policy should remain bounded and testable

### Required CLI memory commands

- `deepagents memory get`
- `deepagents memory query`
- `deepagents memory remember`
- `deepagents memory edit`
- `deepagents memory delete`
- `deepagents memory pin`
- `deepagents memory unpin`
- `deepagents memory explain`
- `deepagents memory rebuild-summaries`
- `deepagents memory audit`

`put` may remain as a low-level compatibility command during migration, but the user-facing target
surface should be semantic lifecycle commands rather than raw key/value mutation.

### Required command behavior

- commands must operate on scoped memory, not global flat keys
- user- and workspace-targeted commands must surface scope in request and response payloads
- destructive lifecycle commands must report what changed, not only success/failure
- provenance inspection must show why a memory exists and where it came from

## Migration From The Current Baseline

### Migration principles

- do not break current privacy guarantees while adding target behavior
- do not require a flag day migration from local file-backed memory
- make derived artifacts clearly derived
- preserve black-box operability during each stage

### Migration stages

#### Stage 0: Stabilize the baseline as compatibility mode

- keep `memory_store.json` plus `AGENTS.md` working
- explicitly mark them as compatibility storage and projection
- preserve current CLI behavior and E2E coverage

#### Stage 1: Introduce the target schema in-process

- add scoped typed records behind a versioned schema layer
- keep the file backend as a compatibility adapter that maps the old record model into the new one
- implement soft lifecycle states even if the old backend still physically stores flat entries

#### Stage 2: Introduce identity resolution and scope-aware reads

- add canonical IDs and access checks
- keep compatibility mode able to synthesize local IDs
- block workspace retrieval until membership checks exist

#### Stage 3: Introduce retrieval packs

- stop relying on raw `AGENTS.md` injection as the primary runtime path
- add a retrieval pack builder and deterministic diagnostics
- keep `AGENTS.md` generation as operator/debug output during transition

#### Stage 4: Introduce structured write flows

- ship explicit remember/edit/pin/unpin/correct flows
- route all memory writes through the write service
- deprecate generic file-edit guidance as the official memory-write path

#### Stage 5: Introduce automatic extraction and consolidation

- add policy-gated auto-memory
- add summary rebuild and stale-marking jobs
- move high-volume episodic compression out of the request path

#### Stage 6: Make structured storage authoritative

- make the operational store the sole source of truth
- keep `AGENTS.md` only as an optional derived export if still useful for local inspection
- retire baseline-only assumptions from the runtime contract

## Staged Implementation Plan

Each tranche below is normative. The work is not complete until its acceptance criteria and tests
are satisfied.

### Tranche 1: Schema And Lifecycle Foundation

Deliver:

- versioned target `MemoryItem` schema
- lifecycle status model
- provenance model
- soft delete and supersession support
- `edit`, `pin`, `unpin`, `delete`, `explain` CLI/API surfaces

Acceptance:

- records carry scope, type, provenance, confidence, salience, pinned, and lifecycle fields
- hard delete is no longer the default user-facing behavior
- old records can be migrated or adapted without data loss

### Tranche 2: Identity And Access

Deliver:

- canonical identity resolution traits
- local compatibility identity adapter
- workspace membership checks
- scope-aware read/write enforcement

Acceptance:

- cross-user reads are blocked in tests
- workspace memory is available only with valid membership
- thread, user, and workspace retrieval paths are independently testable

### Tranche 3: Retrieval Layer

Deliver:

- intent classifier
- query decomposition
- scoped candidate gathering
- ranked retrieval
- deterministic compact memory pack

Acceptance:

- runtime no longer depends on full `AGENTS.md` dump for target-mode retrieval
- ranking explanations are available in diagnostics or audit
- prompt budget remains bounded with summary substitution

### Tranche 4: Explicit Memory Write Protocol

Deliver:

- dedicated runtime memory tool or equivalent structured API
- explicit remember flow with confirmation
- correction and contradiction handling

Acceptance:

- the agent can store memory without raw file editing instructions
- write policy decisions are auditable
- explicit remember defaults to correct scope semantics

### Tranche 5: Automatic Extraction And Consolidation

Deliver:

- policy-gated auto-memory extraction
- dedupe and merge logic
- summary rebuild jobs
- semantic consolidation outputs

Acceptance:

- repeated preferences can consolidate into semantic memory
- low-signal or sensitive data is rejected according to policy
- stale episodic memory can be summarized or demoted

### Tranche 6: Storage And Contract Cleanup

Deliver:

- operational store as source of truth
- vector retrieval integration
- optional blob layer
- compatibility-mode boundaries documented and minimized

Acceptance:

- `AGENTS.md` is no longer required for target-mode runtime correctness
- retrieval, write, and audit flows all operate against canonical structured storage
- compatibility mode remains explicit rather than accidental

## Verification Strategy

### Required test layers

- unit tests for schema, access checks, ranking, dedupe, supersession, and lifecycle rules
- integration tests for operational store adapters, vector adapters, and migration logic
- runtime tests for deterministic pack injection and private-state behavior
- CLI E2E tests for all user-facing lifecycle commands
- black-box multi-identity and multi-workspace tests

### Required target E2E scenarios

- same user across two channels sees the same user-scoped memory
- one user's private memory is invisible to another user
- workspace memory is visible to members and hidden from non-members
- thread memory stays thread-local
- explicit "remember this" stores pinned memory in the expected scope
- contradiction creates supersession rather than silent overwrite
- edit and delete flows preserve auditability
- retrieval ranking prefers pinned and relevant scoped memory
- summary substitution keeps prompt memory within budget
- automatic extraction stores durable high-signal facts and rejects low-signal or sensitive data
- subagents receive bounded derived memory rather than the full parent private corpus

### Required migration tests

- old `memory_store.json` records can be imported or adapted into the new schema
- generated `AGENTS.md` remains inspectable during compatibility stages
- compatibility mode and target mode can coexist without cross-scope leakage

## Open Questions

- Should the first target implementation store embeddings inline or behind a separate embedding
  table/reference layer?
- How much of identity resolution should live inside this repo versus a surrounding product service?
- Should `system` scope ship in the first target release or remain internal-only until later?
- Should workspace memory support subspaces or project partitions in the first release, or only tags?
- What exact provider-visible pack format gives the best balance between determinism, readability,
  and token efficiency?

## Implementation Readiness Summary

The previous baseline-only RFC was not informative enough to support full implementation of
[`memory-design.md`](./memory-design.md). This version is intended to be sufficient for that goal.

It now defines:

- the target entities and durable schema
- the required access-control and identity model
- the retrieval and write pipelines
- lifecycle semantics beyond flat key/value storage
- storage layers and migration rules
- repo seams for implementation
- CLI and runtime requirements
- staged delivery with acceptance criteria
- a verification matrix for the full target architecture

Any future memory implementation work should now be evaluated against this RFC, not against the old
baseline-only contract.
