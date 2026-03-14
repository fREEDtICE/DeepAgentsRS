# Memory Design (Target State)

- Status: Target-state design
- Current implementation baseline: [`memory-architecture-rfc.md`](./memory-architecture-rfc.md)

## Status and scope

This document is intentionally aspirational. It describes the memory architecture DeepAgentsRS could
evolve toward after the current root-scoped, file-backed memory baseline is hardened.

The current repository does **not** yet implement:

* canonical `User` / `ChannelAccount` / `Thread` / `Workspace` identity graphs
* durable `thread` / `user` / `workspace` scope fields on stored memory entries
* typed long-term memory records such as `profile`, `episodic`, `semantic`, `procedural`, and `pinned`
* hybrid relational + vector retrieval with ranked memory packs
* automatic extraction, consolidation, or permissioned workspace sharing

For shipped behavior, constraints, and current guarantees, treat
[`memory-architecture-rfc.md`](./memory-architecture-rfc.md) as authoritative. Read the rest of
this document as a target-state design, not as the current repository contract.

## 1. Design goals

This memory system should support five things at once:

* **Identity continuity**: one user, multiple channels, same personal memory
* **Isolation**: different users cannot see each other’s private memory
* **Workspace collaboration**: users in the same workspace can share selected memory
* **Explicit memory writing**: “remember this” should work reliably
* **Autonomous memory use**: the agent should decide what to retrieve, and sometimes what to store

The cleanest way to do this is to separate memory into **scopes**, **types**, and **lifecycles**.

---

## 2. Core memory model

### 2.1 Memory scopes

Every memory item belongs to exactly one scope:

1. **Thread scope**

   * Tied to one long-lived thread in one channel
   * Useful for channel-specific context, like “in this WhatsApp thread, user prefers short replies”

2. **User scope**

   * Shared across all channels for the same user
   * Private to that user
   * Examples:

     * user’s preferences
     * personal profile
     * long-term goals
     * explicit facts they asked the agent to remember

3. **Workspace scope**

   * Shared by users inside the same workspace
   * Examples:

     * team glossary
     * project milestones
     * shared decisions
     * operating conventions

4. **System/agent scope** (optional but useful)

   * Non-user memory such as policies, retrieval heuristics, or cached tool metadata
   * Not shown as user memory

This scope design maps directly to your requirements:

* same user across channels → **user scope**
* different users isolated → **user isolation**
* shared memory in workspace → **workspace scope**
* one valid thread per channel → **thread scope**

---

### 2.2 Memory types

Within each scope, split memory into different types:

1. **Profile memory**

   * stable facts about an entity
   * examples: name, timezone, role, preferences

2. **Episodic memory**

   * events, interactions, decisions, tasks, conversations
   * examples: “on March 2 user said they are planning a Japan trip”

3. **Semantic memory**

   * distilled knowledge inferred from many episodes
   * examples: “user prefers evening reminders”

4. **Procedural memory**

   * how the agent should behave for this user/workspace
   * examples: “always summarize in bullets”, “when expense > $500, notify workspace admin”

5. **Pinned / explicit memory**

   * memory the user explicitly asked to remember
   * highest trust and retention priority

6. **Working memory**

   * short-lived session state for current reasoning
   * not part of durable long-term memory unless promoted

A good rule:

* **raw interactions become episodic memory**
* **important repeated patterns become semantic/procedural memory**
* **user-commanded memory becomes pinned memory**

---

## 3. Identity and tenancy model

Use a strict identity graph.

### 3.1 Main entities

* `User`
* `ChannelAccount`
* `Thread`
* `Workspace`
* `WorkspaceMembership`
* `MemoryItem`

### 3.2 Identity rules

* A **User** can have multiple channel identities:

  * Telegram account
  * email address
  * Slack DM identity
  * SMS number
  * Lark account
  * QQ account
  * ...etc

* Each channel identity maps to one canonical `user_id`

* Each channel has one long-lived thread per user:

  * `(channel_id, external_thread_key) -> thread_id`
  * if the product guarantees one thread only, you can store one canonical thread per `(user_id, channel)`

* A user can belong to multiple workspaces

### 3.3 Isolation rules

Access to memory is checked by scope:

* `thread` memory: only accessible in that thread context
* `user` memory: accessible only for that user
* `workspace` memory: accessible only if current user is a member of the workspace
* cross-user reads blocked by default

This is important because “same workspace share memory” should not become “everyone sees everything.” Workspace memory should only include items written into workspace scope.

---

## 4. Memory item schema

A durable memory record should look something like this:

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
  "embedding_vector": "...",
  "tags": ["preference", "response-style"],
  "status": "active"
}
```

### Important fields

* **scope_type / scope_id**: who owns it
* **memory_type**: profile, episodic, semantic, procedural, pinned
* **source.kind**:

  * `explicit_user_request`
  * `extracted_from_message`
  * `inferred`
  * `workspace_event`
  * `system_imported`
* **confidence**: how certain the system is
* **salience**: how important it is to retrieve
* **pinned**: user explicitly wants it remembered
* **supersedes**: link old memory to new corrected memory
* **valid_to / status**: handle outdated facts cleanly

---

## 5. Storage architecture

Use a **hybrid memory store**.

### 5.1 Recommended storage layers

Regardless of the storage technology, store **user memory** and **workspace memory** separately (at least as separate logical partitions; physical separation is often better).
This avoids accidental cross-scope leakage, makes permission checks and deletion/retention policies simpler, and keeps retrieval behavior predictable.

Practical patterns:

* **Operational DB**: separate tables or a hard `scope_type` partition with strict row-level policies and scoped indexes
* **Vector index**: separate collections/namespaces (or separate indices) for user vs workspace embeddings
* **Blob store**: separate buckets/prefixes per scope, with distinct ACLs and retention rules
* **Cache**: scope-aware cache keys (never share hot entries across scopes)

1. **Operational database**

   * PostgreSQL or similar
   * source of truth for memory metadata, permissions, versions, links

2. **Vector index**

   * for semantic retrieval
   * stores embeddings of memory content and summaries

3. **Document / blob store**

   * optional for raw conversation snapshots, attachments, long notes

4. **Cache**

   * for hot memory, recent retrievals, user/workspace summaries

### 5.2 Why hybrid

A vector DB alone is not enough because you need:

* permission checks
* explicit pinning
* versioning
* expiration
* conflict resolution
* auditability

A relational DB alone is not enough because you also need:

* semantic similarity retrieval
* fuzzy recall from natural language

---

## 6. Retrieval design

The agent should not dump all memory into every prompt. It should perform **layered retrieval**.

### 6.1 Retrieval order

When a new message arrives, retrieve in this order:

1. **Working memory**

   * current turn/session context

2. **Recent thread memory**

   * last few interactions in the channel thread

3. **Pinned explicit memory**

   * user- or workspace-pinned facts

4. **Relevant user memory**

   * private personal memory across all channels

5. **Relevant workspace memory**

   * only if current context belongs to a workspace

6. **Summaries**

   * if too much detail exists, use summary nodes instead of raw items

### 6.2 Retrieval strategy

Use a weighted retrieval score:

`score = semantic_similarity * a + recency * b + salience * c + pinned_bonus + scope_priority + confidence * d`

Typical priorities:

* pinned memory gets a strong boost
* user memory usually outranks workspace memory for personal requests
* workspace memory outranks personal memory for team/project requests

### 6.3 Query decomposition

Before retrieval, classify the message:

* personal
* workspace/project
* mixed
* command to memorize
* factual recall
* planning/task-related
* preference-sensitive

Then generate multiple retrieval queries:

* direct semantic query from user message
* hidden query for preferences
* hidden query for active goals/tasks
* hidden query for workspace context if in workspace thread

This lets the agent “continuously retrieve memory based on its own strategies.”

---

## 7. Write pipeline

Not every message should become long-term memory. Use a staged pipeline.

### 7.1 Write sources

A memory candidate may come from:

* explicit user instruction: “remember that I’m vegetarian”
* implicit extraction: “my daughter’s birthday is June 3”
* inferred pattern: repeated preference over time
* workspace decision: “Team decided weekly sync is every Tuesday”
* agent-generated consolidation: summaries or distilled preferences

### 7.2 Write stages

1. **Detect candidate**
2. **Classify scope**

   * user or workspace or thread
3. **Classify type**

   * profile / episodic / semantic / procedural / pinned
4. **Safety and privacy filter**
5. **Deduplicate / merge**
6. **Version or supersede**
7. **Store**
8. **Re-embed / update summaries**

### 7.3 Explicit memorization

When user says “remember X”:

* store as **pinned memory**
* prefer **user scope** unless they clearly mean the workspace
* confirm what was memorized
* allow later editing and forgetting

Example:

* “Remember that I prefer Chinese replies” → user scope, pinned, procedural/preference
* “Remember that our team ships every Friday” → workspace scope, pinned, semantic/procedural

---

## 8. Memory extraction policy

You need strong rules for what the agent stores automatically.

### 8.1 Good candidates for auto-memory

* durable preferences
* long-term goals
* recurring habits
* stable personal profile facts
* important project decisions
* workspace norms
* deadlines and commitments with lasting relevance

### 8.2 Bad candidates

* one-off chit-chat
* low-signal emotional reactions unless product explicitly supports this
* sensitive data without consent
* temporary states unless needed operationally
* speculative inferences

### 8.3 Confidence thresholds

Use different thresholds:

* **explicit request**: always store unless policy blocks it
* **strong extracted fact**: store if confidence high
* **inferred preference**: require repeated evidence
* **sensitive content**: require explicit consent or don’t store

---

## 9. Workspace memory model

Shared memory should be deliberate, not accidental.

### 9.1 Workspace memory categories

* shared facts
* project state
* decisions
* conventions
* shared tasks
* shared documents or summaries

### 9.2 Prevent leakage

A message in a workspace context may contain both:

* personal info
* workspace info

So memory write should support **dual extraction**:

* personal fact goes to user scope
* shared decision goes to workspace scope

Example:

* “I’ll be out Friday, and the team agreed to move demo to Monday”

  * “I’ll be out Friday” → user or calendar/task system, not workspace memory unless relevant
  * “team agreed to move demo to Monday” → workspace memory

---

## 10. Forgetting, decay, and correction

A good memory system must forget and revise.

### 10.1 Forgetting mechanisms

1. **Explicit delete**

   * “forget my old address”

2. **Soft decay**

   * lower retrieval priority over time for low-value episodic memories

3. **Expiration**

   * temporary memories can expire automatically

4. **Supersession**

   * “I now live in Singapore” supersedes “I live in Shanghai”

### 10.2 Never hard-delete everything by default

For auditability, use:

* `status = inactive/deleted`
* `supersedes = old_memory_id`

Then apply product/policy rules for actual retention deletion.

---

## 11. Summarization and consolidation

Without consolidation, memory grows noisy.

### 11.1 Periodic consolidation jobs

Run background compaction processes that:

* cluster related episodic memories
* create semantic summaries
* merge duplicate facts
* mark stale memories
* produce “memory cards” like:

  * user preferences summary
  * active goals summary
  * workspace project summary

### 11.2 Example

Raw episodes:

* “Please keep it short”
* “Too much detail”
* “Can you summarize faster next time?”

Consolidated semantic memory:

* “User prefers concise responses by default.”

This is much better for retrieval than dragging many raw episodes into the prompt.

---

## 12. Suggested system components

A practical future service split:

### 12.1 Services

* **Identity Service**

  * maps channels to users
  * manages workspace membership

* **Conversation Service**

  * stores long-lived threads and messages

* **Memory Write Service**

  * extracts, classifies, deduplicates, stores memory

* **Memory Retrieval Service**

  * retrieves ranked memory packs for each turn

* **Memory Consolidation Service**

  * summarizes and merges old memory

* **Policy / Privacy Service**

  * access control, retention, sensitive-data policy

* **Audit Service**

  * track why a memory exists and where it came from

### 12.2 Prompt-facing output

Retrieval service should return a compact package:

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

This prevents raw memory clutter from flooding the model.

---

## 13. Core algorithms

### 13.1 On message receive

```text
1. Resolve channel identity -> user_id
2. Resolve thread_id
3. Detect workspace context if any
4. Classify intent and topic
5. Retrieve relevant memory from:
   working -> thread -> pinned -> user -> workspace
6. Build prompt context
7. Generate reply
8. Evaluate whether new memory candidates should be written
9. Write confirmed candidates
```

### 13.2 On “remember this”

```text
1. Parse target content
2. Infer scope:
   default user scope
   workspace scope if explicitly shared/team-oriented
3. Store as pinned memory
4. Return confirmation
```

### 13.3 On contradiction

```text
1. Find conflicting active memories
2. Prefer newer explicit statements
3. Mark old memory as superseded
4. Recompute summary memory
```

---

## 14. Permission and privacy model

This part matters a lot.

### 14.1 Rules

* User memory is private by default
* Workspace memory is shared only within workspace
* Thread memory does not automatically become user/workspace memory
* Sensitive memory should require stronger storage rules
* The agent should be able to explain:

  * what it remembers
  * why
  * from where
  * how to delete or correct it

### 14.2 Minimum controls for users

Users should be able to:

* see remembered items
* pin/unpin memory
* edit memory
* delete memory
* choose auto-memory settings
* mark memory as personal vs workspace-shared

---

## 15. Recommended data model

A minimal relational model:

### Tables

* `users`
* `channel_accounts`
* `threads`
* `workspaces`
* `workspace_memberships`
* `messages`
* `memory_items`
* `memory_links`
  links memory to messages, entities, superseded memories
* `memory_summaries`
* `memory_access_policies`
* `memory_audit_logs`

### Key indexes

* by `(scope_type, scope_id, status)`
* vector index on memory embeddings
* by `tags`
* by `pinned`
* by `updated_at`
* by `memory_type`

---

## 16. Product behaviors to define early

You should make explicit decisions on these:

1. **Default scope for “remember this”**

   * usually user-private

2. **Can workspace admins inspect shared memory only, or also member-contributed memory provenance?**

3. **Should agent auto-store sensitive personal details?**

   * usually no, unless explicitly asked

4. **Can users opt out of automatic memory creation while keeping explicit memory?**

   * recommended yes

5. **What is the maximum prompt memory budget per turn?**

   * must be bounded

6. **Should workspace memory be separated by project/topic?**

   * often yes, through tags or subspaces

---

## 17. Recommended target architecture

The recommended target architecture is:

* **Canonical identity layer**

  * unify all channels to one `user_id`

* **Three durable scopes**

  * thread, user, workspace

* **Five durable memory types**

  * profile, episodic, semantic, procedural, pinned

* **Hybrid retrieval**

  * relational filters first, vector ranking second, then summarization

* **Memory write pipeline with policy gate**

  * explicit writes always supported
  * automatic writes only for high-signal durable information

* **Consolidation layer**

  * periodically compress episodes into semantic summaries

That gives you strong correctness, isolation, and scale.

---

## 18. A simple example

User sends from Telegram:

> Remember that I’m lactose intolerant.

System stores:

* scope: `user`
* type: `pinned + profile`
* visible across all user channels

Later same user emails:

> Find lunch options for tomorrow.

Retrieval brings in:

* user pinned dietary fact from Telegram-origin memory
* thread/channel doesn’t matter because user scope is shared

Now inside workspace:

> Remember that the design review is moved to Thursday 3pm.

System stores:

* scope: `workspace`
* type: `pinned + semantic/event`
* retrievable by other workspace members

---

## 19. One-sentence principle

**Treat memory as scoped, typed, permissioned knowledge that is selectively written, continuously retrieved, and continuously consolidated.**

Future RFCs should adopt pieces of this design incrementally while preserving the current baseline
guarantees documented in [`memory-architecture-rfc.md`](./memory-architecture-rfc.md).
