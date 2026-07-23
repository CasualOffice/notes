# Casual Note — High-Level Design (HLD)
*Runtime designs, deployable components, interface contracts, and control-flow for the unified local-first notebook.*

**Status:** Canonical downstream of the Design Foundation. This document owns the runtime designs of the hard subsystems and the concrete interface surface (Tauri commands, events, OS adapter traits). It does **not** restate the domain model (see Data Model) or the product rationale (see PRD). Where this document and the Foundation disagree, the Foundation wins and must be amended first.

---

## 1. Objectives & Scope

The HLD turns the Architecture's decomposition into *deployable components, wire-level interfaces, and control flows*. Its objectives:

1. **Component realization.** Map the crate graph to concrete binaries, dynamic native adapters, and Tauri plugins; define the process/thread topology.
2. **Interface contracts.** Specify the public Tauri command surface (the only WebView↔Core door), the `AppEvent` push model, and the OS adapter traits with honest per-platform capability reporting.
3. **Runtime flows.** Define sequence-level designs for the five load-bearing paths: note edit + backlink resolution, global quick-capture, backgrounded reminder firing, meeting capture→artifact→tasks, and unified RAG "ask your notes".
4. **Quality gates.** State the NFR envelope and the acceptance gates each subsystem must pass.

**In scope:** subsystem runtime designs, command/event/adapter contracts, error taxonomy touchpoints, reliability/recovery flows.
**Out of scope (owned elsewhere):** physical SQLite schema and JSON schemas (Data Model); per-feature UX behavior (Feature Specs); milestone sequencing (Roadmap); product scope and personas (PRD).

---

## 2. Non-Functional Requirements

| # | NFR | Target | Verification | Owner subsystem |
|---|-----|--------|--------------|-----------------|
| N1 | Cold launch to interactive | < 2 s | Startup trace, p95 on Tier-2 reference HW | `tauri-app`, `storage` |
| N2 | Recording start latency (arm→first captured frame) | < 100 ms | Capture timestamp delta | `capture-*`, `media-pipeline` |
| N3 | Live transcript lag (speech→on-screen) | 1–2 s | End-to-end wall-clock probe | `speech-whisper`, `media-pipeline` |
| N4 | Keystroke→persist (note autosave) | < 250 ms debounce, no dropped ops | Journal replay test | `notes`, `storage` |
| N5 | Search first paint (FTS path) | < 10 ms | Query bench (BM25 only) | `search` |
| N6 | Hybrid re-fuse (vector stream-in) | < 300 ms | RRF bench | `search`, `embeddings` |
| N7 | Reminder delivery accuracy (app running) | ± 1 s of `fire_at` | Timer-wheel harness | `scheduler` |
| N8 | Reminder delivery (app closed, ≤14-day horizon) | fires within OS granularity | OS trigger integration test | `scheduler` |
| N9 | Peak resident memory (typical, models loaded) | < 3 GB | RSS sampling | `llm-llamacpp`, `speech-*` |
| N10 | Crash recovery: zero lost keystrokes / recorded seconds | 100% | Kill-during-write fault injection | `storage`, journals |
| N11 | Network egress in core paths | 0 bytes | Egress firewall test (only `model-download`/`updater` allowed) | all |
| N12 | Data-at-rest confidentiality | SQLCipher AES-256; key never on disk plaintext | Static + keystore audit | `storage` |
| N13 | RT audio callback discipline | no alloc / lock / DB / inference on callback | TSan + audit lint | `media-pipeline`, `capture-*` |
| N14 | AI answer grounding | 100% of displayed citations resolve to real chunks | Citation-verify gate | `ai-workspace` |

---

## 3. Deployment View

Casual Note ships as a **single desktop binary** per platform plus native audio adapters and signed model assets. No server, no daemon, no account.

```
┌─────────────────────────────── Installed Application ───────────────────────────────┐
│                                                                                       │
│  casualnote(.exe/.app/AppImage)  ── Tauri 2 host process                              │
│    ├── WebView (React/TS bundle, CSP-locked, no network)                              │
│    ├── Rust Core (Tokio multi-thread runtime) ── all domain logic                     │
│    └── Tauri plugins: single-instance, global-shortcut, deep-link(casualnote://),     │
│         autostart, notification, updater, fs(scoped), dialog, os, process             │
│                                                                                       │
│  Native audio adapters (loaded by capture-* crates):                                  │
│    macOS: libcasualnote_capture.dylib ⇄ Swift ScreenCaptureKit XPC-free in-proc       │
│    Windows: WASAPI process-loopback (ActivateAudioInterfaceAsync)                      │
│    Linux: PipeWire/WirePlumber client + xdg-desktop-portal                            │
│                                                                                       │
│  On-disk store (per-user app data dir):                                               │
│    casualnote.db (SQLCipher)    ← single source of truth (all 4 pillars + vectors)    │
│    journals/*.ndjson            ← append-only session + entity_op logs                │
│    files/<sha256[0:2]>/<sha256> ← content-addressed audio + attachments               │
│    models/<registry>/*.gguf|onnx|bin ← signed model registry                          │
│    keystore ref → OS Keychain / Credential Manager / Secret Service (DB key)          │
└───────────────────────────────────────────────────────────────────────────────────────┘

Network boundary (dashed = only two consented services may cross):
    model-download ─ ─→ signed model CDN (SHA-256 + manifest verified)
    updater        ─ ─→ signed release channel (Authenticode / notarize / minisign)
```

**Packaging:** macOS DMG (notarized, hardened runtime, ScreenCapture + microphone entitlements); Windows MSI/EXE (Authenticode); Linux AppImage + Flatpak (portal permissions declared). Models are **not** bundled; first-run downloads or USB-imports the hardware-tier default.

---

## 4. Container / Component View

```
                         ┌──────────────────────── WEBVIEW (untrusted-ish) ────────────────────────┐
                         │  React/TS features:                                                      │
                         │   editor(Tiptap+nodes) · notebooks · daily · tasks · reminders ·         │
                         │   meetings · command-palette(Go/Do/Ask) · search · ai-workspace ·        │
                         │   graph · quick-capture · settings                                       │
                         └───────────▲──────────────────────────────────┬──────────────────────────┘
                                     │ AppEvent (emit)                   │ invoke(cmd, args) → Result
                        ─ ─ ─ ─ ─ ─ ─│─ ─ ─ ─ ─ IPC boundary ─ ─ ─ ─ ─ ─│─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─
                                     │                                   ▼
     ┌──────────────────────────────┴─────────── RUST CORE (Tokio) ─────────────────────────────────┐
     │                                                                                                │
     │   tauri-app  ── command router · event bus · window/tray/activation · capability files         │
     │        │                                                                                        │
     │        ▼                                                                                        │
     │   app-service ── orchestration facade (transactions, cross-crate workflows, event emission)     │
     │        │                                                                                        │
     │   ┌────┼──────────┬──────────┬───────────┬──────────┬───────────┬──────────────┐               │
     │   ▼    ▼          ▼          ▼           ▼          ▼           ▼              ▼               │
     │ notes tasks   reminders  scheduler    links      app-nlp   ai-workspace     search             │
     │   │    │          │          │           │          │           │              │               │
     │   └────┴────┬─────┴────┬─────┴─────┬─────┴────┬─────┴────┬──────┴──────┬───────┘               │
     │             ▼          ▼           ▼          ▼          ▼             ▼                        │
     │          storage   embeddings   model-manager  export   app-domain (shared types/errors)       │
     │             │          │           │                                                            │
     │             ▼          ▼           ▼                                                            │
     │        rusqlite +  sqlite-vec   signed registry                                                 │
     │        SQLCipher   (in-DB)      + download/updater (only net owners)                            │
     │                                                                                                 │
     │   ── Meeting subsystem (safety-critical, isolated threads) ──                                   │
     │   capture-api → capture-macos/windows/linux → ring buffers → media-pipeline →                   │
     │   speech-api(speech-whisper | speech-parakeet) → llm-api(llm-llamacpp) → INDEXING → spine       │
     │                                                                                                 │
     │   sync-core ── DORMANT in v1 (op-log/HLC/UUID projection seam only)                             │
     └─────────────────────────────────────────────────────────────────────────────────────────────┘
```

**Threading model:** (a) WebView thread; (b) Tokio worker pool for async domain/IO; (c) **dedicated native capture threads** writing bounded lock-free ring buffers — never touched by Tokio, never allocating; (d) a **single-resident LLM context thread** with a bounded request queue; (e) STT worker(s). Raw PCM never crosses IPC or serializes to JSON (N13). The LLM never owns recording state.

---

## 5. Rust Workspace / Crate Layout (extended)

```
casualnote/
├─ crates/
│  ├─ app-domain        # shared types, ID (UUIDv7/ULID), HLC, error taxonomy, event enums
│  ├─ app-service       # orchestration facade; owns transactions + AppEvent emission
│  ├─ tauri-app         # command router, event bus, windows/tray/activation, capabilities
│  │
│  ├─ notes             # Tiptap JSON parse, block-index projection, [[wiki]]/#tag/@mention, MD i/o
│  ├─ tasks             # areas/projects/headings/tasks/checklists; derived buckets; frac-index reorder
│  ├─ reminders         # reminder entity + state machine; rrule recurrence advance
│  ├─ scheduler         # dual-layer notifier (timer-wheel + OS one-shot + catch-up sweep)
│  ├─ links             # polymorphic edge table, backlinks, unlinked-mention, neighborhood graph
│  ├─ app-nlp           # hybrid grammar/regex + LLM-fallback natural-language entry → ParsedEntry
│  ├─ embeddings        # trait + embeddings-gemma / embeddings-bge; sqlite-vec integration
│  ├─ ai-workspace      # retrieve→RRF→rerank→grounded-decode(AnswerV1)→citation-verify; suggestions
│  ├─ search            # FTS5(BM25) ∪ sqlite-vec KNN, RRF fuse, filter compilation
│  │
│  ├─ capture-api       # unified capture trait + capability report
│  ├─ capture-macos     # Swift ScreenCaptureKit adapter
│  ├─ capture-windows   # WASAPI process-loopback adapter
│  ├─ capture-linux     # PipeWire/WirePlumber + portal adapter
│  ├─ media-pipeline    # downmix/resample→16k mono f32, DC/gain, VAD, chunking, drift
│  ├─ speech-api        # STT trait (two-pass live+final, model profiles)
│  ├─ speech-whisper    # whisper.cpp adapter (default)
│  ├─ speech-parakeet   # ONNX Parakeet TDT v3 "Turbo (English)" opt-in adapter
│  ├─ llm-api           # LLM trait; GBNF-constrained structured output; repair→fallback
│  ├─ llm-llamacpp      # llama.cpp/GGUF (Qwen3), MLX on Apple Silicon
│  │
│  ├─ storage           # rusqlite + SQLCipher; NDJSON journals; content-addressed blobs
│  ├─ model-manager     # signed manifests, SHA-256, resumable download, offline import
│  ├─ export            # Markdown/PDF/artifact export
│  └─ sync-core         # DORMANT: op-log/HLC/UUID substrate + projection seam
└─ ui/                  # React/TS feature modules (see Foundation §5)
```

Dependency direction is strictly downward: feature crates depend on `storage`/`app-domain`; `app-service` composes them; `tauri-app` is the only crate exposing `#[tauri::command]`. `sync-core` is compiled but not wired into write paths in v1.

---

## 6. Public Tauri Command Surface

All commands are `async`, return `Result<T, AppError>` (typed/retryable taxonomy from baseline), and validate arguments against `app-domain` types before touching `storage`. The WebView **never** sees SQL, file paths outside the scoped attachments dir, or raw PCM. Commands are grouped; each carries a per-window capability requirement.

| Command | Args (abridged) | Returns | Notes |
|---------|-----------------|---------|-------|
| **Notes & blocks** | | | |
| `notes.create` | `{notebook_id?, daily_date?, doc_json?}` | `NoteId` | seeds blockIds; projects index |
| `notes.get` | `{note_id}` | `Note{doc_json, meta}` | |
| `notes.save` | `{note_id, doc_json, base_version}` | `SaveResult{version}` | schema-validated; optimistic-concurrency; triggers projection |
| `notes.list` | `{notebook_id?, filter?}` | `NoteSummary[]` | |
| `notes.delete` | `{note_id}` | `()` | soft-delete tombstone |
| `notes.resolveLinks` | `{note_id}` | `LinkResolution[]` | wikilink target resolution incl. create-on-miss stubs |
| `blocks.get` | `{block_id}` | `Block` | evidence/backlink target |
| `blocks.backlinks` | `{block_id\|note_id}` | `Backlink[]` | derived-on-read from `links` |
| **Tasks / projects / areas** | | | |
| `tasks.create` | `{title, project_id?, start_on?, deadline_on?, checklist?}` | `TaskId` | |
| `tasks.update` | `{task_id, patch}` | `Task` | per-field LWW-by-HLC |
| `tasks.complete` | `{task_id, at}` | `Task` | advances recurrence if bound |
| `tasks.reorder` | `{task_id, before?, after?}` | `order_key` | fractional index |
| `tasks.bucket` | `{bucket: Today\|Upcoming\|Anytime\|Someday}` | `Task[]` | **derived query**, not stored state |
| `projects.create` / `areas.create` | `{name, ...}` | `Id` | |
| **Reminders & scheduling** | | | |
| `reminders.create` | `{target_ref, fire_at, tz, rrule?}` | `ReminderId` | schedules into both layers |
| `reminders.snooze` | `{reminder_id, until}` | `Reminder` | cancels+reschedules OS handle |
| `reminders.cancel` | `{reminder_id}` | `()` | cancels OS handle |
| `reminders.upcoming` | `{horizon?}` | `Reminder[]` | |
| **Quick capture & NLP** | | | |
| `capture.quick` | `{text, kind_hint?}` | `CaptureResult{entity_ref, parsed}` | routes via `app-nlp` |
| `nlp.parse` | `{text}` | `ParsedEntry` | live-highlight preview; no side effects |
| **Meeting** | | | |
| `meeting.preflight` | `{sources[]}` | `PreflightReport{capability}` | honest capability report |
| `meeting.start` | `{sources[], note_binding?}` | `SessionId` | NEW→…→RECORDING |
| `meeting.pause` / `meeting.resume` | `{session_id}` | `SessionState` | |
| `meeting.stop` | `{session_id}` | `SessionState` | →STOPPING→CAPTURED |
| `meeting.artifact` | `{session_id}` | `MeetingArtifactV1` | after GENERATING |
| `meeting.actionItemToTask` | `{session_id, action_item_id, overrides?}` | `TaskId` | writes `spawned_from` link + evidence |
| **Search & AI** | | | |
| `search.query` | `{q, filters?, mode}` | `SearchHits` (streamed) | FTS sync, vector re-fuse |
| `palette.run` | `{mode: Go\|Do\|Ask, input}` | mode-specific | command palette entry |
| `ai.ask` | `{question, scope?}` | `AnswerV1` | grounded, citation-verified |
| `ai.suggestions.list` / `.apply` / `.dismiss` | `{...}` | `Suggestion[]` / `()` | reversible, user-approved |
| **Models & export** | | | |
| `models.list` / `.install` / `.import` / `.remove` | `{...}` | `ModelInstallation[]` | signed; resumable; USB import |
| `models.selectTier` | `{tier}` | `()` | hardware-tier override |
| `export.note` / `export.artifact` | `{id, format}` | `path` | scoped fs |

---

## 7. Event Model (`AppEvent`)

The Core pushes state to the WebView via a single typed event channel (`tauri::Window::emit`). Events are **derived facts**, never commands; the WebView reconciles its local view. All variants carry a monotonic `seq` and originating `entity_ref` where applicable.

```rust
enum AppEvent {
  // Notes / knowledge
  NoteSaved { note_id, version, changed_block_ids: Vec<BlockId> },
  NoteProjected { note_id },                 // block-index/FTS/links rebuilt
  BacklinksChanged { target_ref, count },
  TagChanged { tag_id },

  // Tasks / planning
  TaskChanged { task_id, bucket_hint },
  TaskCompleted { task_id, recurrence_spawned: Option<TaskId> },
  ProjectChanged { project_id },

  // Reminders / scheduler
  ReminderScheduled { reminder_id, fire_at, os_layer: bool },
  ReminderFired { reminder_id, target_ref, grouped: bool },
  ReminderMissedSwept { reminder_ids: Vec<ReminderId> },  // catch-up

  // Meeting lifecycle
  SessionStateChanged { session_id, from: State, to: State, degraded: Option<Reason> },
  CaptureLevel { session_id, rms_dbfs },     // throttled UI meter
  LiveTranscript { session_id, segment: TranscriptSegment },  // pass-1 stream
  ArtifactReady { session_id },
  IndexingProgress { session_id, stage, pct },

  // AI / search
  SearchPartial { query_id, hits, source: Fts | Vector },    // stream + re-fuse
  AnswerReady { query_id },
  SuggestionsReady { batch_id, count },

  // System / models / health
  ModelDownloadProgress { model_id, pct, resumable: bool },
  CapabilityReport { platform_caps: PlatformCaps },
  OfflineReady { ok: bool },
  Error { taxonomy: ErrorClass, retryable: bool, context },
}
```

---

## 8. Sequence Diagrams

### 8.1 Create / edit a note with backlink resolution

```
WebView(editor)   tauri-app     notes            links          storage        (emit)
     │  save(doc_json,base_ver) │                 │              │
     ├───────────────────────►  │ notes.save      │              │
     │                          ├───────────────► │ validate schema, check base_ver
     │                          │                 │ project blocks (assign missing blockIds)
     │                          │                 ├────────────► append entity_op (HLC) + write note row (txn)
     │                          │                 │ extract [[wiki]]/#tag/@mention from doc_json
     │                          │                 ├──► links.reconcile(note_id, refs)
     │                          │                 │        │ resolve targets by title;
     │                          │                 │        │ create stub note for unresolved [[X]]
     │                          │                 │        ├─────────► upsert edges (OR-Set, no dual-write)
     │                          │                 │ update FTS5 + queue embedding (content-hash gated)
     │                          │ ◄───────────────┤ SaveResult{version}
     │ ◄────────────────────────┤                 │
     │        emit NoteSaved, NoteProjected, BacklinksChanged(for each resolved target)
     │ ◄══════════════════════════════════════════════════════════════════════════════
     │ editor reconciles; backlinked notes' panels refresh on next read (derived-on-read)
```

Backlinks are **never dual-written**: the edge lives once (`src_block_id → dst`), and the target's "linked references" panel is a `links.backlinks` read. Embedding is debounced and content-hash-gated so re-saves without body change don't re-embed.

### 8.2 Global quick-capture of a task/note

```
OS hotkey     global-shortcut   capture window   tauri-app     app-nlp        tasks/notes     storage
   │  ⌘⇧Space      │                              │
   ├────────────►  │ toggle capture window (frameless, always-on-top, skipTaskbar)
   │               ├─────────────►  focus input   │
   │  user types "call Sam tomorrow 3pm #work !2"  │
   │               │ (live) nlp.parse ───────────► │ grammar tokenize → ParsedEntry{date,proj,prio}
   │               │ ◄─────────── highlighted preview (no side effects)
   │  Enter        │                              │
   │               │ capture.quick(text) ───────► │ route by kind_hint/ParsedEntry
   │               │                              ├─ high conf: task (start_on=tomorrow 15:00, prio=2, #work)
   │               │                              │   (low conf → resident Qwen3 schema-constrained fallback)
   │               │                              ├──────────────────────────► write entity_op + rows (txn)
   │               │                              │   thread onto today's daily note (daily_date)
   │               │                              │   if time present → reminders.create → scheduler
   │               │ ◄──────────── CaptureResult  │
   │               │ hide window, toast confirm   │
   │        emit TaskChanged / NoteSaved / ReminderScheduled
```

Target: input-focused in <150 ms; parse preview is pure (no writes) so cancel is free.

### 8.3 Reminder firing while app is backgrounded

```
── At schedule time (app running) ──
reminders.create → scheduler:
   Layer A: insert into Tokio timer-wheel/min-heap keyed on fire_at
   Layer B: if fire_at within 14-day horizon AND platform supports →
            register one-shot OS trigger (UNCalendarNotificationTrigger / ScheduledToastNotification)
            store os_handle on reminder row     [Linux: no OS layer — capability reported honestly]
   persist reminder(state=pending) to SQLite (durable truth)

── Fire while app BACKGROUNDED / CLOSED ──
   OS scheduler ──fires──► native notification shown by OS (no core process needed)
        │
   user clicks action (e.g. "Snooze 10m" / "Open")
        │
   deep-link casualnote:// ──► single-instance ──► app wakes / focuses
        │
   scheduler de-dup: read reminder.state;
        if still 'pending' → flip pending→fired (Layer that fires first wins; other no-ops)
        route action: snooze → cancel os_handle, reschedule (both layers); open → focus target_ref
   emit ReminderFired{grouped:false}

── App was CLOSED past fire_at (missed) ──
   on launch / wake-from-sleep: scheduler sweeps  state='pending' AND fire_at < now
        coalesce → ONE grouped notification, mark those rows 'missed', surface in-app inbox
   emit ReminderMissedSwept + ReminderFired{grouped:true}
```

De-dup is **state-gated**, not timer-gated, so Layer A and Layer B firing near-simultaneously never double-notifies. Any mutation (edit/complete/snooze/cancel) cancels the stored `os_handle` first, preventing stale OS fires.

### 8.4 Meeting capture → transcribe → artifact → action-items → tasks

```
meetings UI   tauri-app   capture-*   ring buf   media-pipeline  speech-api   llm-api   app-service   storage/spine
   │ preflight │
   │ start(sources) → SessionState machine: NEW→PREFLIGHT→READY→RECORDING
   │            │  native capture thread ──PCM──► bounded lock-free ring (no alloc/lock on callback)
   │            │                          ring ──► downmix/resample 16k mono f32, DC/gain, VAD, chunk
   │            │                                        │ overlapping chunks
   │            │                                        ├──► pass-1 live (whisper base) → LiveTranscript(stream)
   │            │  (session journal: append-only NDJSON per captured second — crash-safe)
   │ stop ────► │  RECORDING→STOPPING→CAPTURED
   │            │  CAPTURED→FINAL_TRANSCRIBING: pass-2 (small/medium) → TranscriptSegment[] (t_start/t_end/speaker)
   │            │  →GENERATING: llm-api builds MeetingArtifactV1 under GBNF grammar
   │            │      (schema-validated; 1 repair; else deterministic fallback; NO invented owners/dates)
   │            │      every fact carries evidence_segment_ids[]
   │            │  →INDEXING: write note(from artifact) + segments as transcript-segment blocks;
   │            │             chunk (30–60s VAD/turn) + embed; upsert links; FTS; sqlite-vec
   │            │  →COMPLETE
   │   emit SessionStateChanged*, ArtifactReady, IndexingProgress
   │
   │ review artifact → user picks action_item → meeting.actionItemToTask(id, overrides?)
   │            │  create entity(task)+task row;
   │            │  link(task → meeting, rel='spawned_from', evidence_segment_ids=[…])
   │            │  if discussed in a note block → link(task → block, rel='about')
   │            │  carry owner→assignee, due_date→deadline_on  ONLY if model extracted from evidence
   │   emit TaskChanged
   │  → task now shows "From meeting Q3 Planning (00:14:22) → jump to transcript evidence"
```

INDEXING is the unification stage: a meeting becomes a note, its segments become addressable evidence blocks, and its action items become one-click tasks whose provenance rides on the *edge* (survives later task edits).

### 8.5 Unified AI "ask your notes" retrieval

```
palette(Ask)  tauri-app   ai-workspace   search        embeddings    llm-api       storage
   │ ?"what did we decide about pricing?"
   ├────────► ai.ask(question, scope?)
   │                       │ embed(question) ──────────► query vector (256-dim int8)
   │                       │ retrieve (parallel):
   │                       │    ├─ search.query FTS5(BM25) ─────────────► candidate chunks (<10ms)
   │                       │    └─ search.query sqlite-vec KNN ────────► candidate chunks
   │                       │ compile filters (type:/tag:/date:/person:/is:) → SQL predicates pre-fusion
   │                       │ fuse: Reciprocal Rank Fusion (no score normalization)
   │                       │ optional bge-reranker over top-K
   │                       │ build grounded prompt with NUMBERED evidence (breadcrumb-carried chunks)
   │                       │ constrained-decode AnswerV1{answer, citations[], confidence, unanswered}
   │                       │ CITATION-VERIFY: every citation must resolve to a real chunk
   │                       │    if none resolve → return unanswered:true ("I couldn't find this in your notes")
   │ ◄──────── AnswerV1 (only verified citations shown; each links chunk→note block / segment+timestamp)
   │   emit SearchPartial(stream), AnswerReady
```

The answer spans all four pillars because retrieval runs over the universal `chunk`/`entity` spine (note blocks, transcript windows, tasks, reminders). Refusal-over-hallucination is a hard gate (N14): an unverifiable citation is dropped; an answer with zero verifiable citations becomes `unanswered`.

---

## 9. OS Adapter Contracts

### 9.1 Capture trait (`capture-api`)

```rust
trait CaptureAdapter: Send {
    fn capabilities(&self) -> PlatformCaps;         // honest, per-platform
    fn enumerate_sources(&self) -> Result<Vec<CaptureSource>, CaptureError>; // apps / mic / system
    fn preflight(&self, sel: &SourceSelection) -> Result<PreflightReport, CaptureError>;
    fn start(&mut self, sel: &SourceSelection, sink: RingSink) -> Result<CaptureHandle, CaptureError>;
    fn pause(&mut self, h: &CaptureHandle) -> Result<(), CaptureError>;
    fn resume(&mut self, h: &CaptureHandle) -> Result<(), CaptureError>;
    fn stop(&mut self, h: CaptureHandle) -> Result<(), CaptureError>;
}
```

`RingSink` writes into a bounded lock-free ring; the adapter's native thread must never allocate, lock, block on IO, or call into inference/DB on the audio callback (N13). PCM is never serialized to JSON or sent across IPC.

| Platform | Backend | App-level audio | Exclude-self | System fallback | Notes |
|----------|---------|-----------------|--------------|-----------------|-------|
| macOS | ScreenCaptureKit (`SCShareableContent`/`SCContentFilter`/`SCStream`) | Yes | Yes (exclude-self) | n/a | needs ScreenCapture entitlement + TCC |
| Windows | WASAPI process-loopback (`ActivateAudioInterfaceAsync`, incl. process tree) | Yes | Yes | **Never silent system-wide** | reports if process-loopback unavailable |
| Linux | PipeWire registry/streams + WirePlumber; xdg portals on Wayland | Yes (node-level) | Best-effort | n/a | portal grant required; capability honest |

`PlatformCaps` is surfaced to the UI (`CapabilityReport` event) so the app never pretends to a capability it lacks (e.g. **Linux has no OS-notification scheduling layer** — reported, not faked).

### 9.2 Speech & LLM traits

```rust
trait SpeechAdapter { fn profiles(&self)->Vec<ModelProfile>;
    fn transcribe_live(&mut self, chunk: &AudioChunk) -> Vec<PartialSegment>;
    fn transcribe_final(&mut self, audio: &AudioSpan) -> Result<Vec<TranscriptSegment>, SpeechError>; }

trait LlmAdapter { fn generate_constrained<T: Schema>(&mut self, prompt: &str, gbnf: &Grammar)
    -> Result<T, LlmError>;  /* one repair then deterministic fallback */ }
```

Adapters (`speech-whisper`/`speech-parakeet`, `llm-llamacpp`) are swappable behind these traits. LLM uses a single resident context + bounded request queue; GBNF grammars hard-constrain `MeetingArtifactV1` and `AnswerV1`.

### 9.3 Notification/scheduler OS contract

| Platform | OS one-shot layer (Layer B) | Handle type | Honest fallback |
|----------|------------------------------|-------------|-----------------|
| macOS | `UNCalendarNotificationTrigger` | request identifier | — |
| Windows | `ScheduledToastNotification` | tag/group | — |
| Linux | **none** | — | Layer A only; capability reported; relaunch-to-fire is post-v1 |

---

## 10. Reliability, Error Taxonomy & Observability

- **Journals:** append-only NDJSON for (a) recording sessions (per-second crash safety) and (b) `entity_op` mutations. Derived tables — block index, `links`, FTS5, sqlite-vec — are **fully rebuildable** from `note.doc_json` + `entity_op` log (N10).
- **State-machine safety:** session states include DEGRADED/FAILED/RECOVERING; a failed transcription/LLM stage never loses captured audio (falls back to `CAPTURED` with retry).
- **Error taxonomy:** every `AppError` is classified `{class, retryable}` and surfaced via `AppEvent::Error`; retryable classes (transient IO, model-not-loaded, capture-glitch) get bounded retry; terminal classes surface actionable UI.
- **Concurrency safety:** `notes.save` uses `base_version` optimistic concurrency; structured entities use per-field LWW-by-HLC; tags/links use OR-Set semantics — so the dormant `sync-core` seam needs no re-model later.
- **Observability without telemetry:** structured local logs + in-app health panel (capability report, model status, offline-ready flag, last recovery). **Zero network egress** in any of this (N11).

---

## 11. Quality Gates (acceptance)

| Gate | Criterion |
|------|-----------|
| G1 Launch | Cold launch < 2 s to interactive on Tier-2 reference HW (N1) |
| G2 Capture | Arm→first frame < 100 ms; no alloc/lock on RT callback (TSan clean) (N2, N13) |
| G3 Transcript | Live lag 1–2 s; two-pass final segments time-anchored (N3) |
| G4 Persist | No lost keystroke/second under kill-during-write fault injection (N10) |
| G5 Search | FTS first paint < 10 ms; vector re-fuse < 300 ms (N5, N6) |
| G6 Scheduler | ± 1 s while running; OS-layer fire when closed within 14-day horizon; no double-notify; missed-sweep coalesces (N7, N8) |
| G7 Grounding | 100% displayed citations resolve; zero-citation → `unanswered` (N14) |
| G8 Privacy | 0 bytes egress in core paths; SQLCipher at rest; key only in OS keystore (N11, N12) |
| G9 Capability honesty | UI never exposes a platform capability the adapter reports absent |
| G10 Recovery | Derived indexes fully rebuildable from `doc_json` + `entity_op`; DEGRADED/FAILED never lose source data |

---

*End of High-Level Design. This document is subordinate to the Design Foundation and coordinates with — without duplicating — the PRD (product), Architecture (decomposition/security/packaging), Data Model (physical schema/JSON schemas), Feature Specs (per-feature behavior), and Roadmap (sequencing).*
