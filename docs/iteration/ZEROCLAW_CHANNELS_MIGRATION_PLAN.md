# Plan: Foundation-First Migration of ZeroClaw Channels into DeepAgentsRS

## Summary

- Build a new channels foundation inside `DeepAgentsRS`, then port every transport from `zeroclaw/src/channels/*` onto it in waves.
- Keep DeepAgentsRS as the execution core; do not port `run_tool_call_loop(...)` or ZeroClaw provider internals.
- Preserve transport UX parity for channels: per-conversation continuity, typing indicators, draft streaming, reactions, attachment rendering/parsing, cancellation-on-new-message where supported, and channel-specific reply formatting.
- Include `start_channels`/`doctor_channels` equivalents, the shared transcription/TTS helpers needed by channel transports, and TOML config compatibility for `[channels_config.*]`.
- Exclude gateway/webhook and cron from this migration plan.

## Public APIs and Config

- Add `crates/deepagents-platform` with the transport-neutral seam:
  - `SessionKey` newtype for channel conversation identity.
  - `InboundTurn` containing `session_key`, `channel`, `sender`, `reply_target`, `thread_id`, normalized text, optional `ContentBlock`s, and transport metadata.
  - `TurnResult` containing `status`, `final_text`, `messages`, `state`, and optional interrupt payload.
  - `SessionSnapshot` containing persisted DeepAgents `Vec<Message>` plus `AgentState`.
  - `AgentEngine` trait with `run_turn(input, snapshot, sink) -> TurnResult`.
  - `SessionStore` trait with in-memory default implementation.
- Add `crates/deepagents-channels` with trait-segregated channel capabilities instead of a god trait:
  - `ChannelTransport` for `name`, `listen`, `send`, `health_check`.
  - `TypingControl`, `DraftControl`, `ReactionControl`, and `PinControl` as optional capability traits.
  - `ChannelBinding` struct that composes a transport with any supported optional capabilities.
  - Shared transport models: `InboundEnvelope`, `OutboundEnvelope`, `DeliveryHandle`, `AttachmentRef`, `ChannelFeatures`.
- Port TOML channel config blocks under a new `ChannelServerConfig`:
  - Preserve ZeroClaw field names and section names for `[channels_config.*]`.
  - Add a smaller DeepAgents-owned wrapper for `root`, provider selection, model, API key/base URL, runtime timeouts, summarization, and feature flags.
  - Keep optional compile features aligned with current ZeroClaw split: `channel-matrix`, `channel-lark`, `whatsapp-web`.

## Implementation Changes

1. Build the engine boundary first.
   - Implement `DeepAgentsEngine` in `deepagents-platform` using existing `SimpleRuntime`/`ResumableRunner` and `RunEventSink`.
   - Persist channel conversations as DeepAgents `Message` history plus `AgentState`; do not reuse ZeroClaw `ChatMessage` caches.
   - Default session identity to `channel:<channel>:<reply_target>:<thread_or_default>:<sender>` so DM, group, and threaded conversations do not collide.

2. Build a channel dispatcher/runtime in `deepagents-channels`.
   - Port the supervised listener model, shared inbound bus, bounded concurrency, per-session cancellation token map, and health-doctor flow.
   - Map runtime events to UX actions: `AssistantTextDelta -> update_draft`, `RunFinished -> finalize/send`, errors/cancel -> cancel draft and swap completion reaction, typing lifecycle scoped to each turn.
   - Use DeepAgents summarization/offload/session history as the continuity mechanism; do not re-port ZeroClaw’s ad hoc history compaction logic.

3. Port shared transport utilities before concrete transports.
   - Move shared helpers for attachment markers, tool-tag stripping, outbound sanitization, transcription, TTS, chunking, and workspace-safe attachment persistence into channel-agnostic modules.
   - Normalize inbound media into `InboundTurn`:
     - Images become `ContentBlock`s when the transport can provide stable local paths/URLs.
     - Non-image attachments remain textual markers.
     - Voice/audio pre-processing stays adapter-side and feeds text into the turn before engine execution.
   - Keep outbound marker rendering transport-specific for Telegram/Discord/QQ and any other channel that needs upload vs inline URL decisions.

4. Port concrete transports in waves, with each module adapted to `ChannelBinding`.
   - Wave 1: `matrix`, `signal`, `nextcloud_talk`, `linq`, `wati`, `qq`, `dingtalk`, `lark`, `nostr`.
   - Wave 2: `cli`, `telegram`, `slack`, `discord`, `mattermost`, `irc`, `email_channel`.
   - Wave 3: `whatsapp`, `whatsapp_web`, `imessage`, `clawdtalk`.
   - Keep per-platform files largely intact where possible, but refactor constructors/config parsing to remove direct dependency on ZeroClaw config/runtime/provider modules.

5. Expose runnable entrypoints in the current workspace.
   - Extend `deepagents-cli` with `channels serve --config <path>` and `channels doctor --config <path>`.
   - `doctor` and `serve` must use the same config loader and channel factory path so health and runtime startup stay in sync.
   - Do not implement ZeroClaw runtime commands (`/models`, `/model`) or config hot-reload in the initial migration; if a later phase needs them, build them on top of `SessionStore` and the new config wrapper rather than copying the old mixed logic.

## Test Plan

- Foundation tests:
  - `SessionKey` derivation for DM/thread/group inputs.
  - `SessionStore` round-trip for `messages + state`.
  - `DeepAgentsEngine` preserves prior history across turns and forwards `RunEvent`s unchanged.
- Dispatcher tests:
  - draft updates stream and finalize correctly;
  - typing always stops on success, error, and cancellation;
  - reactions swap from ack to success/error;
  - a second inbound message cancels an in-flight turn when that channel enables interruption.
- Behavior parity tests:
  - no raw tool-call JSON or XML-like tool tags leak to outbound replies;
  - follow-up turns restore per-session history;
  - channel-specific system instructions are injected once and kept in the correct order;
  - attachment-only inbound messages are normalized the same way as ZeroClaw per transport.
- Transport conformance tests:
  - each ported channel passes `send`, `listen`, and `health_check` smoke tests;
  - Telegram chunking/attachment markers, Slack attachment persistence, Discord upload marker parsing, Matrix voice/media flow, WhatsApp mode split, and feature-gated builds all have targeted regression tests.
- End-to-end tests:
  - mock-provider channel runs for Matrix, Signal, Nextcloud Talk, Linq, WATI, QQ, DingTalk, Lark, and Nostr as first-wave reference transports;
  - one smoke test per remaining transport constructor/factory path;
  - `channels doctor` reports configured healthy/unhealthy/timeout states deterministically.

## Assumptions and Defaults

- “Foundation first” means the new `AgentEngine`/`SessionStore`/dispatcher lands before bulk transport file migration.
- “Transport UX parity” does not include ZeroClaw provider aliasing, semantic-memory autosave, runtime config hot reload, or `/models`/`/model` command parity in the initial migration.
- TOML compatibility is preserved for the channel blocks themselves, not for the entire ZeroClaw application config surface.
- The migrated channel server uses the providers already supported by DeepAgentsRS; broad ZeroClaw provider portability is out of scope for this channels plan.
- HITL interrupts are surfaced as terminal channel outcomes in the initial migration; interactive approval/resume over channel transports is deferred.
