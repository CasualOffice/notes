# Casual Note — System Architecture Specification

*How the pieces of a fully-local, four-pillar notebook fit together — and why. Extends the inherited EchoNote meeting-intelligence architecture to notes, tasks, reminders, and a unified brain.*

**Status:** Authoritative for system decomposition. Subordinate to the Design Foundation (the canonical spine). Where this document would contradict the Foundation, the Foundation wins and this document is corrected. This document owns the *how the pieces fit and why*; it does **not** define table columns (see Data Model), per-feature behavior (see Feature Specs), runtime subsystem designs (see HLD), or product rationale (see PRD).

---

## 1. Architectural Position & Principles

### 1.1 Position

Casual Note is a **local desktop system, not a hosted SaaS**. It is a warm, everyday notebook — notes, tasks, reminders — that also happens to be a best-in-class local meeting recorder and understander. Four pillars (Notes, Reminders, Tasks, Meetings) share **one encrypted local store, one search index, one link graph, and one AI workspace**. The device is the primary and only copy in v1; the cloud is never assumed and is touched only by two named, user-consented services (model download, app update).

This extends the inherited EchoNote architecture rather than replacing it. The capture stack, media pipeline, speech/LLM engine traits, session state machine, model manager, storage-and-encryption spine, and packaging pipeline are **carried forward unchanged in intent**. What is new is everything a notebook adds: a JSON-doc editor with a Rust-side projection pipeline, a task/reminder domain, a durable dual-layer notification scheduler, a polymorphic link graph, hybrid RRF search, and an evidence-cited AI workspace — all built with the same local-first, crash-safe, privacy-by-design rigor.

### 1.2 Principles (carried forward and extended)

| # | Principle | Architectural consequence |
|---|-----------|---------------------------|
| P1 | **Local-first** | SQLite (SQLCipher) is the single source of truth for notes, tasks, reminders, *and* meetings. No server round-trip on any core path. |
| P2 | **Privacy-by-design** | No telemetry by default. Encryption at rest; keys in OS keystore. Only `model-download` and `updater` may open a socket. |
| P3 | **Offline-capable** | Write, plan, record, transcribe, reason, search all function with the NIC disabled. "Offline Ready" is a testable, enforced state. |
| P4 | **Zero-config** | First launch yields a default notebook, today's daily note, and an auto-selected hardware-appropriate model tier. No account, no wizard. |
| P5 | **Crash-safe** | Append-only NDJSON journals for recording sessions *and* entity mutations. Every derived table (block index, links, FTS, vectors) is fully rebuildable from source (`note.doc_json`, `entity_op` log). No keystroke or recorded second is ever lost. |
| P6 | **Unified** | One search, one link graph, one AI workspace, one store across four pillars. A thin universal `entity` spine + per-type detail tables + one polymorphic `link` table. |
| P7 | **Capture is safety-critical & native** | Audio capture stays independent of the WebView lifecycle. Raw PCM never crosses into the WebView or is serialized to JSON. |
| P8 | **Rust owns semantics; the WebView is a thin view** | All parsing, projection, link extraction, scheduling, retrieval, and inference live in Rust. The WebView edits JSON and renders events. |
| P9 | **Honest capability reporting** | Platform differences (e.g. Linux has no OS-level scheduled-notification layer) are surfaced in the UI, never hidden behind a leaky abstraction. |

---

## 2. System Context

```
+============================== USER DESKTOP =====================================+
|                                                                                 |
|   Selected-app audio    Microphone     Keyboard / editor      Global hotkey     |
|          |                  |                 |                    |             |
|          +--- Native Capture Adapters ---+    |            Quick-Capture panel   |
|          |   (SCK / WASAPI / PipeWire)   |    |            (frameless window)     |
|          +---------------+---------------+    |                    |             |
|                          |                    |                    |             |
|                 +--------v--------+   +--------v--------------------v--------+   |
|                 | Rust Media Pipe |   |   Tauri IPC command surface         |   |
|                 | (16k mono, VAD) |   |   (typed, deny-by-default caps)     |   |
|                 +--------+--------+   +--------+----------------------------+   |
|                          |                     |                                |
|   +----------------------v---------------------v-----------------------------+  |
|   |                       RUST CORE (Tokio async runtime)                      | |
|   |  Session coord | Notes | Tasks | Reminders | Scheduler | Links | app-nlp   | |
|   |  Speech engine | LLM engine | Embeddings | ai-workspace | Search | Storage | |
|   +----------------------------------+----------------------------------------+  |
|                                      |                                          |
|             +------------------------v-------------------------+                |
|             |   SQLCipher DB  +  content-addressed files  +    |                |
|             |   NDJSON journals  +  signed model registry       |                |
|             +--------------------------------------------------+                |
|                                      |                                          |
|          OS keystore (key)   OS notification center   System tray               |
|                                                                                 |
+=================================================================================+
                                       |
                 Network permitted ONLY for two named services:
                 (1) model download   (2) signed application update
```

The WebView receives only meters, state events, projected view models, and text — never raw PCM and never SQL. The Rust core is the trust and correctness boundary.

---

## 3. Logical Component Architecture

Inherited components are marked *(carried)*; new pillars are marked *(new)*. Each maps to one or more crates from the Foundation's Module/Crate Map (§5).

| Component | Responsibilities | Crate(s) | Tech |
|-----------|------------------|----------|------|
| Desktop shell *(carried)* | Window lifecycle, tray, activation policy, capability policy, global shortcut | `tauri-app` | Tauri 2 + native WebView |
| Frontend *(carried/ext)* | Editor, notebooks, daily, tasks, reminders, meetings, palette, search, AI, graph, quick-capture, settings | `ui/*` | React/TypeScript + Tiptap |
| Session coordinator *(carried)* | Meeting state machine, cancellation, recovery, event sequencing | `app-service`, `app-domain` | Rust + Tokio |
| Capture service *(carried)* | App enumeration, permission flow, PCM capture, device-change handling | `capture-api`, `capture-{macos,windows,linux}` | Native adapters |
| Media pipeline *(carried)* | Clock alignment, downmix, resample→16k mono f32, DC/gain, VAD, chunking, drift | `media-pipeline` | Rust; ring buffers; rubato/dasp |
| Speech service *(carried/ext)* | Streaming + final decode, stabilization, language detect | `speech-api`, `speech-whisper`, `speech-parakeet` | whisper.cpp / ONNX / Apple |
| Language service *(carried)* | Structured extraction (MeetingArtifactV1), grounded answering (AnswerV1), transforms | `llm-api`, `llm-llamacpp` | llama.cpp / GGUF / MLX |
| **Notes service** *(new)* | Tiptap JSON validation, block-index projection, `[[wikilink]]`/`#tag`/`@mention` extraction, Markdown import/export | `notes` | Rust |
| **Task service** *(new)* | Areas/projects/headings/tasks/checklists, derived-bucket queries, fractional-index reorder | `tasks` | Rust |
| **Reminder service** *(new)* | Reminder entity, recurrence advance (`rrule`), reminder state machine | `reminders` | Rust + `rrule` crate |
| **Scheduler service** *(new)* | Dual-layer notification scheduler (timer-wheel + OS one-shot handoff + catch-up sweep + de-dup) | `scheduler` | Rust + Tokio + OS APIs |
| **Link / Graph service** *(new)* | Polymorphic edge table, derived backlinks, unlinked-mention resolution, neighborhood graph | `links` | Rust |
| **NL entry** *(new)* | Grammar/regex fast path + LLM-fallback for dates/`every[!]`/`#project @tag !priority` | `app-nlp` | Rust + resident LLM |
| **Embeddings** *(new)* | Trait + adapters; incremental content-hash-gated embedding; sqlite-vec integration | `embeddings` | Rust + ONNX/GGUF |
| **AI workspace** *(new)* | Retrieve → RRF → optional rerank → grounded decode → citation-verify → suggestions | `ai-workspace` | Rust |
| Search *(carried/ext)* | Hybrid FTS5 (BM25) ∪ sqlite-vec KNN, RRF fusion, filter compilation | `search` | SQLite FTS5 + sqlite-vec |
| Persistence *(carried/ext)* | DB access, journals, content-addressed files, projection writes | `storage` | `rusqlite` + SQLCipher |
| Model manager *(carried)* | Signed manifests, checksums, resumable download, disk preflight, offline import | `model-manager` | Rust HTTP (explicit only) |
| Export *(carried)* | Markdown/HTML/JSON; optional DOCX/PDF | `export` | Rust |
| **Sync seam** *(new, dormant)* | UUIDv7/ULID + HLC + append-only `entity_op` log; tables as materialized projection | `sync-core` | Rust (inert in v1) |

### 3.1 Editor ↔ Rust sync contract (new — the notebook's critical path)

The editor is the single most-touched surface, so its persistence contract is defined explicitly. The WebView **edits JSON only**; Rust owns all derived structure.

```
WebView (Tiptap/ProseMirror)                 Rust core (`notes` + `storage`)
  edit transaction
     |  debounce (~250ms idle / on-blur / on-window-hide)
     v
  notes.save(note_id, doc_json, base_version) ---> validate against PM schema
                                                  |  reject if invalid (typed err)
                                                  v
                                            persist doc_json (SQLCipher txn)
                                                  |  append entity_op (HLC)
                                                  v
                                            PROJECT: walk doc, ensure every
                                            block node has stable blockId,
                                            upsert block index rows
                                                  |
                                                  v
                                            EXTRACT links: [[wikilink]] / #tag /
                                            @mention / embeds -> link table
                                            (unlinked mentions resolved lazily)
                                                  |
                                                  v
                                            FTS5 upsert (per block/heading chunk)
                                                  |
                                                  v
                                            enqueue embedding job (content-hash
                                            gated, debounced) -> sqlite-vec
                                                  |
  <--- note_saved{version, projected_block_ids, resolved_links} ----+
```

Contract guarantees: (1) `doc_json` is validated before any write; (2) the save is transactional — block index, links, and FTS commit atomically with the doc, embeddings follow asynchronously; (3) `base_version` provides optimistic-concurrency detection so a stale WebView cannot clobber a newer doc (relevant when quick-capture and the main window both target the daily note); (4) block IDs are **stable and Rust-assigned when missing**, so backlinks, reminders-on-a-block, and evidence anchors survive re-edits.

---

## 4. Process & Thread Model

One signed desktop application, one Tokio runtime, native inference libraries in-process (optional supervised sidecar retained for problematic GPU drivers). The inherited rule holds and is **extended to the notebook**: *no DB transaction, allocation-heavy work, or inference runs on the RT audio callback — and no synchronous projection/embedding runs on the IPC dispatch thread.*

```
 WebView main thread (React + Tiptap)
        |  typed IPC (no SQL, no PCM)
        v
 Tauri IPC dispatcher  ------------------------------------------------+
        |                                                              |
        v                                                              |
 Tokio multi-thread runtime                                            |
   +-- Session coordinator (meeting state machine, one per session)    |
   +-- Notes projection worker  (save -> project -> link -> FTS)  [NEW]|
   +-- Indexer worker           (embedding jobs, idle-time suggestions)[NEW]
   +-- Reminder scheduler task  (timer-wheel min-heap on fire_at)  [NEW]
   +-- app-nlp                  (grammar fast path; LLM fallback)  [NEW]
   +-- Persistence writer       (single writer; serialized txns)       |
   +-- Model manager            (network-owning; explicit only)        |
        |                                                              |
   +----+--------------------+--------------------+                    |
   | Capture RT thread(s)     | STT worker pool    | LLM worker         |
   | (native, no blocking I/O)| bounded queue      | single resident    |
   | -> lock-free ring buffer | (live + final)     | ctx + req queue    |
   +--------------------------+--------------------+                    |
        |                                                              |
   Background writer: audio chunks + session NDJSON + entity_op NDJSON  |
                                                                       <+
```

### 4.1 Concurrency contracts

| Concern | Rule |
|---------|------|
| **DB writer** | Single logical writer serializes all mutating transactions (SQLite WAL, `rusqlite`). Readers use additional connections; `busy_timeout` + retry. Projection, scheduler advance, and meeting-INDEXING writes all funnel through it. |
| **RT audio** | Native capture threads only write timestamped frames into bounded lock-free ring buffers. Never allocate, never touch DB, never call inference, never cross the WebView. *(carried)* |
| **Editor persistence** | Save is debounced in the WebView; validation + projection run on a Tokio worker, never blocking the IPC dispatcher. Embedding is a separate, content-hash-gated, further-debounced job on the indexer. |
| **Scheduler** | The timer-wheel is an in-memory min-heap owned by one Tokio task; it is *derived* and rebuilt from SQLite on boot. All snooze/edit/advance mutations go through the DB writer first, then patch the heap. |
| **LLM** | Single resident context, bounded request queue. Meeting generation, ask-your-notes answering, and NL-entry fallback contend on the same queue; the LLM **never owns recording state**. *(carried)* |
| **Backpressure** | Every cross-thread channel is bounded; overflow degrades gracefully (drop live-transcript refresh, defer embeddings) rather than growing unbounded. |

---

## 5. Cross-Platform Capture Architecture *(carried forward, condensed)*

Per-application audio capture semantics differ fundamentally across OSes, so capture lives behind a unified Rust trait with native adapters and **honest capability reporting**. Raw PCM stays in native memory and local files.

```rust
pub trait CaptureAdapter: Send {
    fn capabilities(&self) -> CaptureCapabilities;         // honest per-platform report
    fn enumerate_sources(&self) -> Result<Vec<AudioSource>, CaptureError>;
    fn preflight(&self, req: &CaptureRequest) -> Result<Preflight, CaptureError>;
    fn start(&mut self, req: CaptureRequest, sink: RingSink) -> Result<CaptureSession, CaptureError>;
    fn pause(&mut self) -> Result<(), CaptureError>;
    fn resume(&mut self) -> Result<(), CaptureError>;
    fn stop(&mut self) -> Result<CaptureSummary, CaptureError>;
}
```

| Platform | Mechanism | Notes |
|----------|-----------|-------|
| **macOS** | Swift **ScreenCaptureKit** — `SCShareableContent` enumeration, `SCContentFilter`, `SCStream` for application-level audio; `excludesCurrentProcessAudio` to exclude self | TCC permission flow; visible indicator; native Swift bridged to Rust |
| **Windows** | **WASAPI process-loopback** via `ActivateAudioInterfaceAsync` incl. process tree | Never a silent system-wide fallback — degrade honestly if per-process loopback unavailable |
| **Linux** | **PipeWire** registry/streams + **WirePlumber**; portals on Wayland | Capability-report node availability; portal-consent on Wayland |

Downstream of all three, the `media-pipeline` normalizes identically: monotonic clocks, channel downmix, resample to 16 kHz mono float32, DC removal/gain, VAD, overlapping chunking, drift correction — feeding the STT and LLM schedulers. The **INDEXING** stage of the session state machine now additionally writes captured meetings into the unified entity spine, `chunk` table, `link` table, and vector index (see HLD for stage internals; see §6 below for how notifications are unaffected by capture).

---

## 6. Local Notification / Scheduler Architecture (new — across three OSes)

Reminders and task alerts must survive **both app-running and app-closed** states, without a background daemon (a v1 non-goal is relaunch-quit-app OS scheduler integration). The Foundation mandates a **dual-layer, belt-and-suspenders** scheduler. SQLite is durable truth; the in-memory heap is derived.

```
                     SQLite: reminder rows (fire_at UTC, tz, rrule?, state, snoozed_until, os_handle)
                                          |  (authoritative durable truth)
             +----------------------------+----------------------------+
             |                                                         |
   LAYER A (while running)                               LAYER B (survives app-closed)
   Tokio timer-wheel / min-heap on fire_at              OS one-shot registration within
   - rebuilt from SQLite on launch                      a rolling 14-day horizon
   - owns snooze, edit, rich actions,                   - macOS: UNCalendarNotificationTrigger
     unlimited reminders, recurrence advance            - Windows: ScheduledToastNotification
   - fires -> deliver via tauri notification            - Linux: NONE (reported honestly)
             |                                            - store os_handle; cancel on mutation
             +----------------------------+----------------------------+
                                          |
                          DE-DUP: delivery gated on reminder.state
                          whichever layer fires first flips pending->fired;
                          the other no-ops.
                                          |
             MISSED-REMINDER CATCH-UP (on launch AND on wake-from-sleep):
             sweep state='pending' AND fire_at < now
               -> coalesce into ONE grouped notification
               -> mark 'missed', surface in-app inbox
```

### 6.1 Scheduler trait sketch

```rust
pub trait OsNotificationBackend: Send {
    fn capability(&self) -> SchedulerCapability;   // e.g. { scheduled_while_closed: bool, horizon_days: u16 }
    fn schedule(&self, r: &ScheduledReminder) -> Result<OsHandle, SchedulerError>;
    fn cancel(&self, handle: &OsHandle) -> Result<(), SchedulerError>;
    fn reconcile(&self, active: &[ScheduledReminder]) -> Result<(), SchedulerError>; // re-sync 14d horizon
}

pub enum SchedulerCapability {
    Full { horizon_days: u16 },     // macOS, Windows
    RunningOnly,                    // Linux — honest downgrade; Layer A only
}
```

| OS | Layer B backend | While-closed delivery | Honest report |
|----|-----------------|-----------------------|---------------|
| macOS | `UNCalendarNotificationTrigger` (UserNotifications) | Yes, within 14-day horizon | Full |
| Windows | `ScheduledToastNotification` (Windows.UI.Notifications) | Yes, within 14-day horizon | Full |
| Linux | *none* (freedesktop notifications are running-time only) | No | `RunningOnly` — UI states "reminders fire only while Casual Note is open"; optional autostart mitigates |

**Invariants:** (1) any reminder mutation (create/edit/snooze/complete/delete/recurrence-advance) writes SQLite first, then patches Layer A's heap, then reconciles Layer B's horizon (cancel stale `os_handle`, register new). (2) The 14-day horizon is re-swept on every launch and wake so Layer B never drifts. (3) Recurrence uses the `rrule` crate with **materialize-on-completion** — the template plus the next materialized instance are scheduled, not a pre-expanded series. (4) Snooze sets `snoozed_until` and re-enqueues without losing the original `fire_at` provenance.

---

## 7. Storage & Encryption

One encrypted store holds everything; a crash never loses source-of-truth data.

```
+------------------------------------------------------------------+
| SQLCipher database (whole-DB encryption, WAL mode)               |
|   entity spine  |  per-type detail tables  |  polymorphic link   |
|   block index   |  chunk  |  FTS5 shadow tables  |  sqlite-vec    |
|   entity_op log (HLC)  |  model registry  |  session metadata     |
+------------------------------------------------------------------+
        ^ key (never in DB)                    ^ derived, rebuildable
        |                                      | (block index, links, FTS, vectors)
+-------+---------+          +-----------------+------------------+
| OS keystore     |          | Content-addressed files (SHA-256):  |
| Keychain /      |          |  attachments, captured audio        |
| Credential Mgr /|          +-------------------------------------+
| Secret Service  |          | Append-only NDJSON journals:         |
+-----------------+          |  session journal + entity_op journal |
                             +--------------------------------------+
```

| Aspect | Decision |
|--------|----------|
| DB engine | SQLite via **direct `rusqlite`** in the `storage` crate — **not** `tauri-plugin-sql`. The WebView never sees SQL or raw FS. |
| Encryption | Whole-DB **SQLCipher**; master key in OS keystore (Keychain / Credential Manager / Secret Service), never persisted to the DB or logs. |
| Source of truth | `note.doc_json`, structured detail rows, and the append-only `entity_op` log. All indexes (block, link, FTS5, sqlite-vec, chunk) are **derived and fully rebuildable**. |
| Vector index | **sqlite-vec inside the SQLCipher store** — no external FAISS/usearch. Preserves single encrypted store, transactional consistency, one-file backup. Embeddings Matryoshka-truncated to 256 dims + int8, `embed_model` recorded per chunk. |
| Attachments/audio | Content-addressed (SHA-256) files beside the DB; referenced by hash from `attachment`/`audio_track` rows. |
| Journals | Two append-only NDJSON journals: the meeting **session journal** (carried) and the **entity_op journal** (new). Both are crash-recovery substrate. |
| Backup | Single-file DB + content-addressed blob dir = a coherent snapshot; no server, no partial-cloud state. |
| Sync seam | Every mutable entity carries a stable **UUIDv7/ULID + HLC**; detail tables are a *materialized projection* of `entity_op`. Dormant in v1; enables a later Loro re-encode without a re-model. |

---

## 8. Network Isolation & Model Management

**Only two components may open a socket**, and only with explicit user consent: `model-download` (in `model-manager`) and `updater`. Every other crate is built and tested against a network-disabled harness; "Offline Ready" is an automated test target.

**Language-aware, on-demand model download.** Models are not bundled; they are fetched (resumable, SHA-256-verified, signed-manifest) only when the user consents. Each catalog manifest declares a `LanguageSupport` (multilingual, or `Only(["en", …])` for language-specialized variants like Whisper `*.en`). The registry selects a pack by **(hardware tier × the user's language)** — the user's language comes from the OS locale or an explicit setting: a Whisper `*.en` model is preferred for an English user (smaller/faster/more accurate), a multilingual Whisper for any other language, and multilingual instruct LLMs (Qwen3-class) and a multilingual embedder (bge-m3-class) cover the user's language for summaries/extraction/search. With auto-detect on, a multilingual STT is kept so mixed-language speech still transcribes. So "download the model for *his* language" is a first-class flow: `model_manager::select_pack(catalog, tier, LanguagePreference)` → the right STT/LLM/embedder ids → on-demand fetch. Changing language later just downloads the additional pack.

```
   All core paths (write, plan, record, transcribe, reason, search)
        |
        |  NO network. Ever.
        v
   +--------------------------------------------------------------+
   |  Network-permitted island (isolated, capability-gated):       |
   |    model-manager  --->  signed manifests, SHA-256 checksums,  |
   |                          resumable range downloads,           |
   |                          disk preflight, offline USB import   |
   |    updater        --->  Authenticode / notarized signed builds |
   +--------------------------------------------------------------+
```

| Aspect | Decision |
|--------|----------|
| Ownership | Network I/O owned exclusively by `model-download` and `updater` services. Tauri HTTP/allowlist capabilities scoped to just these. |
| Integrity | Signed manifests + SHA-256 per artifact; resumable range downloads; disk-space preflight before write. |
| Offline install | Models importable via USB / local file; every core path works with network disabled. |
| Model registry | Signed on-device registry of installed STT/LLM/embedder models (`ModelInstallation`), with `embed_model` provenance propagated to every chunk. |
| Tier selection | On first launch, hardware probe auto-selects LLM Tier 1 (4B) / Tier 2 (8B) / Tier 3 (14B), STT profile, and embedder (EmbeddingGemma-300M or bge-base). MLX on Apple Silicon. |
| Engines behind traits | `speech-api` (whisper.cpp default; Parakeet ONNX turbo; Apple SpeechTranscriber), `llm-api` (Qwen3 via llama.cpp/GGUF), `embeddings` — all swappable, all offline after install. |

---

## 9. Security & Privacy Threat Model (extended for notes/tasks/quick-capture/deep-links)

The inherited threat model (least-privilege Tauri capabilities, encrypted secrets, integrity-checked models, no telemetry, no hidden recording, visible capture indicator) is carried forward. It is extended for the notebook surface.

| Threat | Mitigation |
|--------|-----------|
| **Notes/tasks/reminders at rest exposed** (theft, backup leak) | Whole-DB SQLCipher; key in OS keystore, never in DB/logs. Content-addressed attachments live inside the encrypted boundary's directory; audio is local-only. |
| **WebView compromise / XSS in note content** | Strict CSP; the WebView holds no key and issues no SQL; all persistence goes through typed IPC commands validated in Rust. `doc_json` is schema-validated before persist; rendered note HTML is sanitized. Malicious pasted content cannot reach the FS or DB directly. |
| **IPC surface abuse** | Deny-by-default per-window capability files. The `main` and `capture` windows get distinct, minimal command allowlists. No raw-SQL, no raw-FS command is exposed; `fs` plugin scoped to the attachments dir only. |
| **Quick-capture panel spoofing / data leak** | The frameless `capture` window is a first-class app window (not a web surface), created at startup, `skipTaskbar`, always-on-top; it shares the same trust boundary and the same encrypted store. Global hotkey is user-rebindable; capture writes go through the same validated `notes.save`/`capture.quick` commands. |
| **Deep-link injection** (`casualnote://`) | `deep-link` inputs are treated as **untrusted**: parsed and validated in Rust against a strict allowlist of intents (open entity by UUID, start capture), never `eval`'d, never used to construct SQL or FS paths directly. Unknown/malformed links are rejected with a typed error. `single-instance` registered first so links route to the running instance. |
| **Malicious model file** | Signed manifests + SHA-256 verification before load; refuse-and-report on mismatch. |
| **Silent AI edits to user data** | Auto-link/auto-tag are **reversible `suggestion` rows**, cited and user-approved — never silent mutations. The AI never writes to notes/tasks without an explicit accept. |
| **Hallucinated facts presented as truth** | Every AI fact carries transcript/block evidence; citations are **verified to resolve to a real chunk** before display; if none resolve, return `unanswered:true`. The model must not invent owners or dates. |
| **Telemetry / exfiltration** | No telemetry by default. No network on any core path. Observability is local-only (§11). |
| **Reminder/notification content leakage on lock screen** | Notification payloads use minimal content by default; sensitive detail is fetched in-app after unlock (setting-controlled). |

**Trust boundaries (outermost → innermost):** untrusted external (network, deep links, imported files, pasted content) → validating Rust core (schema + allowlist checks) → single DB writer → encrypted store (key held only in OS keystore). The WebView sits *outside* the SQL/key boundary by design.

---

## 10. Reliability & Recovery

The crash-safety guarantee is extended from recordings to **every mutation**.

| Failure | Detection | Recovery |
|---------|-----------|----------|
| App crash mid-edit | On launch, unflushed WebView state is minimal (debounced saves are frequent); last committed `doc_json` + `entity_op` log is authoritative | Reopen at last committed doc version; no keystroke beyond the last debounce window is lost. |
| App crash mid-recording | Session NDJSON journal + audio chunks on disk | Resume post-processing from last checkpoint; recording seconds are never lost. *(carried)* |
| Corrupted/inconsistent derived index | Integrity check on launch; version stamps | Rebuild block index, links, FTS5, and sqlite-vec **from `doc_json` + `entity_op`** — derived tables are disposable. |
| Missed reminders while closed | Launch/wake catch-up sweep (`state='pending' AND fire_at<now`) | Coalesce into one grouped notification, mark `missed`, surface in the in-app inbox. |
| Layer B / OS scheduler drift | Horizon reconcile on every launch/wake | Cancel stale `os_handle`s, re-register the 14-day horizon from SQLite truth. |
| LLM generation fails | Session state machine `DEGRADED`/`FAILED`; schema-validate + one repair then deterministic fallback | Recording and transcript are preserved regardless; artifact can be regenerated. The LLM never blocks recording state. *(carried)* |
| Optimistic-concurrency conflict (two windows edit daily note) | `base_version` mismatch on `notes.save` | Reject stale write with typed error; WebView reloads latest and re-applies. |
| Embedding job failure | Content-hash gate; job idempotent | Retry on next idle sweep; search degrades to FTS-only meanwhile (still functional). |

**State-machine backbone (carried, extended):** `NEW→PREFLIGHT→READY→RECORDING↔PAUSED→STOPPING→CAPTURED→FINAL_TRANSCRIBING→GENERATING→INDEXING→COMPLETE` plus `DEGRADED/FAILED/RECOVERING`. INDEXING now also writes into the unified entity spine, chunk table, links, and vector index. Detailed stage internals belong to the HLD.

---

## 11. Observability Without Telemetry

No data leaves the device. Observability serves the *local* user and support, not a vendor.

| Facility | Behavior |
|----------|----------|
| Structured logs | Local rotating logs, redaction of note/transcript content by default; opt-in verbose mode for diagnostics. Never network-shipped. |
| Local metrics | In-app health panel: model tier, memory, capture latency, index freshness, embedding backlog, scheduler horizon status, offline state. |
| Capability report | Settings surface shows honest per-platform capabilities (e.g. Linux reminder limitation, capture mechanism, model tier). |
| Crash artifacts | Local crash dumps + last journal offsets, retained locally; user may attach manually to a support request — never auto-sent. |
| Search/AI debuggability | Retrieval traces (which chunks, RRF ranks, citations) viewable locally to explain an answer, aiding trust without telemetry. |

---

## 12. Packaging & Release

One signed desktop app per platform; native inference libraries linked in-process (optional supervised sidecar retained for problem GPU drivers).

| Platform | Package | Signing | Notes |
|----------|---------|---------|-------|
| macOS | DMG | **Notarized** (Developer ID) + hardened runtime | Dynamic activation policy: Accessory when idle → Regular when main/meeting window opens. TCC entitlements for capture/notifications. |
| Windows | MSI / EXE | **Authenticode** | WASAPI process-loopback; ScheduledToast for Layer B; autostart opt-in. |
| Linux | AppImage + Flatpak | Signed artifacts | PipeWire/WirePlumber; portals on Wayland; scheduler reports `RunningOnly`. |

| Aspect | Decision |
|--------|----------|
| Windows/tray | Two windows + tray: `main` (chrome) and `capture` (frameless, always-on-top, `skipTaskbar`, created-at-startup, toggled by global hotkey). Per-window deny-by-default capability files. |
| Tauri plugins | `single-instance` (registered first), `global-shortcut`, `deep-link` (`casualnote://`), `autostart` (opt-in), `notification` (delivery surface only), `updater`, `fs` (scoped to attachments), `dialog`, `os`, `process`. **Skip `tauri-plugin-sql`.** |
| Updates | `updater` service only; signed builds; user-consented; no silent background network beyond update/model checks. |
| Model delivery | Models are **not** bundled in the installer beyond an optional first-run download prompt; importable offline via USB. |

---

## 13. Key Decisions & Non-Decisions

### 13.1 Locked decisions (this document's stance, consistent with Foundation §4/§6)

1. **Rust owns all semantics; the WebView is a thin JSON/event view.** No SQL, no PCM, no key in the WebView.
2. **One encrypted store (SQLCipher) for all four pillars**, with sqlite-vec inside it — single-file backup, transactional consistency.
3. **JSON-doc-as-truth + Rust-side projection** for notes: `doc_json` is source; block index / links / FTS / vectors are derived and rebuildable.
4. **Dual-layer scheduler** (Tokio timer-wheel + OS one-shot handoff + catch-up), with **honest capability downgrade on Linux**.
5. **Universal entity spine + one polymorphic link table**; bidirectionality derived on read, never dual-written.
6. **Network isolated to two named services**; every core path offline-testable.
7. **Op-log/HLC/UUID sync seam built now, inert in v1** — the highest-leverage cheap-now decision; enables Loro/CRDT + blind relay later without a re-model.
8. **Single DB writer**, bounded channels, RT-audio and IPC-dispatch both kept off heavy work.
9. **Deep links and imports are untrusted input**, validated against strict allowlists in Rust.

### 13.2 Non-decisions / explicitly out of scope for this document

- **Table columns, constraints, exact JSON schemas** (MeetingArtifactV1 / AnswerV1 / ParsedEntry) → **Data Model**.
- **Runtime subsystem internals** (state-machine stage bodies, RRF/rerank pipeline steps, media-pipeline DSP, model-tier probe algorithm) → **HLD**.
- **Per-feature behavior** (backlink UX, task buckets, recurrence `every`/`every!`, palette three modes, suggestion review UI) → **Feature Specs**.
- **Product rationale, personas, success metrics, prioritization** → **PRD**.
- **Milestone sequencing and acceptance gates** → **Roadmap**.

### 13.3 v1 non-goals (carried from Foundation, restated for architecture)

No multi-device sync / CRDT engine / collaboration / accounts; no mobile; no cloud or third-party AI APIs; no global force-directed graph as a headline feature (neighborhood only); no plugin SDK / public API; no calendar/email integration; no web clipper; no real-time collaborative editing; no relaunch-quit-app OS scheduler integration (later enhancement — hence Linux's honest running-only reminder limitation in v1).

---

*End of System Architecture Specification. Subordinate to the Design Foundation; consistent with the inherited EchoNote architecture and HLD baselines.*
