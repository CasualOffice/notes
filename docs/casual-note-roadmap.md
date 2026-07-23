# Casual Note — Roadmap, Delivery Plan & Test Strategy
*Sequencing, workstreams, milestones, acceptance gates, testing, risks, and open decisions for building the unified local notebook.*

**Status:** Downstream document. Governed by the Design Foundation (canonical spine) and the inherited EchoNote Architecture/HLD baseline. This document owns **sequencing only** — it introduces no scope beyond what the PRD and Feature Specs define, and it defers physical schema to the Data Model and subsystem runtime designs to the HLD. Where this document and the Foundation disagree, the Foundation wins and this document is corrected.

---

## 1. Purpose & Scope

This roadmap answers three questions: **in what order do we build Casual Note, why that order, and how do we know each stage is done and correct.** It covers the phased feature roadmap, the parallel delivery workstreams and their dependencies, milestone acceptance gates with performance targets, a comprehensive test strategy, a risk register, and the open decisions that must be resolved by prototype or benchmark before their dependent work commits.

**Out of scope (owned elsewhere):** table columns and JSON schemas (Data Model); the runtime design of the scheduler, RAG pipeline, and session state machine (HLD); per-feature UX behavior (Feature Specs); product rationale and personas (PRD).

**Guiding constraint for all sequencing:** the *seam* work — universal entity spine, `entity_op`/HLC/UUID op-log, block-IDed documents, capability-honest platform contracts — is built **before** the features that ride it, even though its payoff (sync, rebuild-from-log, cross-pillar links) lands later. Deferring the seam is the one thing that would force a re-model rather than a re-encode.

---

## 2. Sequencing Rationale — Why Notebook-First

Casual Note *leads with the notebook*. The delivery order mirrors the product identity, not the inherited codebase's center of gravity.

1. **The notebook is the everyday surface and the store's spine.** Notes, the daily-note capture thread, blocks, and the universal `entity`/`link` tables are what every other pillar attaches to. A task links to a note block; a reminder targets a note; a meeting *becomes* a note. Building the store and editor first means every later pillar plugs into a real spine instead of a stub.
2. **Meeting intelligence is inherited and de-risked, so it can wait without risk.** The capture stack, media pipeline, whisper.cpp adapter, and MeetingArtifactV1 schema already exist in the EchoNote baseline. That maturity is exactly why meetings are Phase 2, not Phase 1: we are not racing to prove the hard native subsystem works, we are integrating a known-good one into a new spine. Phase 1 must instead prove the *new* risks — JSON-doc-as-truth, derived buckets, the dual-layer scheduler.
3. **Value compounds only after the spine exists.** "Ask your whole knowledge base" (Phase 3) is worthless with an empty base. Semantic search and the AI workspace need notes, tasks, and meetings already flowing into the `chunk` table. Shipping RAG before content exists would demo well and retain no one.
4. **Risk is front-loaded onto the untried, not the proven.** The genuinely novel bets — JSON-doc-as-truth + block projection, Things-style derived buckets, the belt-and-suspenders scheduler, and the op-log seam — are all Phase 1. We want them under real use for two phases before sync (Phase 4) tries to lean on the seam.
5. **Each phase is independently shippable.** Phase 1 alone is a credible local notebook + tasks + reminders (an Obsidian/Things competitor). Every subsequent phase is additive, never a rewrite.

---

## 3. Phased Roadmap

```
 PHASE 1                PHASE 2                 PHASE 3                  PHASE 4 (optional)
 Core Notebook          Meeting                 Semantic Brain           Reach
 + Plan + Store         Intelligence            + AI Workspace           + Sync

 ┌───────────────┐      ┌───────────────┐       ┌───────────────┐        ┌───────────────┐
 │ store + oplog │      │ capture x3 OS │       │ embeddings    │        │ E2E sync      │
 │ Tiptap editor │─────▶│ media pipe    │──────▶│ sqlite-vec    │───────▶│ (Loro+relay)  │
 │ blocks/links  │      │ whisper STT   │       │ hybrid RRF    │        │ OCR / attach  │
 │ tasks buckets │      │ MeetingArtV1  │       │ ai-workspace  │        │ Parakeet turbo│
 │ reminders     │      │ action→task   │       │ suggestions   │        │ integrations  │
 │ scheduler A+B │      │ FTS5 search   │       │ neighborhood  │        │ plugin SDK    │
 │ FTS5 search   │      │ command Go/Do │       │ graph, Ask    │        │ (post-v1)     │
 └───────────────┘      └───────────────┘       └───────────────┘        └───────────────┘
   SHIP: notebook         SHIP: local             SHIP: unified            SHIP: multi-device
   + plan (v0.1)          meeting mind (v0.5)      brain (v1.0)             (v2.x)
```

### Phase 1 — Core Notebook, Planning & Local Store *(the foundation; v0.1)*
The everyday product, fully offline, no AI, no meetings.
- **Store & seam:** SQLCipher via `rusqlite`; universal `entity` spine + per-type detail tables + polymorphic `link` table; `entity_op` append-only op-log with UUIDv7/ULID + HLC; NDJSON entity-mutation journal; content-addressed attachments; OS-keystore key management.
- **Editor:** Tiptap/ProseMirror with `blockId`-stamped custom nodes (todo, callout, code, table, `[[wikilink]]`, `#tag`, `@mention`); `doc_json`-as-truth; schema validation before persist; block-index projection and link extraction in the Rust `notes` crate; Markdown import/export.
- **Notebooks & dailies:** nested notebook/folder tree; daily-note capture spine keyed by `daily_date`; quick-capture frameless panel + global hotkey.
- **Planning:** areas/projects/headings/tasks/checklists; derived Today/Upcoming/Anytime/Someday buckets; `start_on`/`deadline_on` split; fractional-index drag-reorder.
- **Reminders & scheduler:** first-class polymorphic reminders; recurrence via `rrule` (`every`/`every!`); dual-layer scheduler (Tokio timer-wheel + OS one-shot handoff on macOS/Windows, honest "no OS layer" on Linux); missed-reminder catch-up sweep.
- **Search:** FTS5/BM25 over the spine; command palette **Go** + **Do** modes.
- **NL entry:** `app-nlp` grammar/regex fast path with live highlighting (LLM fallback deferred to Phase 2 when a resident model exists).

### Phase 2 — Meeting Intelligence, Summaries & Text Search *(the inherited differentiator; v0.5)*
- **Capture:** carry forward ScreenCaptureKit / WASAPI process-loopback / PipeWire adapters behind the unified trait; ring buffers, RT discipline, media pipeline (16 kHz mono, VAD, chunking, drift) unchanged.
- **STT:** whisper.cpp two-pass (live base / final small-medium) behind `speech-api`.
- **LLM & artifacts:** llama.cpp/GGUF Qwen3, GBNF-constrained MeetingArtifactV1 (summary/topics/decisions/action-items/risks/open-questions), one repair then deterministic fallback; every fact carries `evidence_segment_ids`.
- **Session state machine:** full `NEW→…→COMPLETE` (+ DEGRADED/FAILED/RECOVERING) with the **INDEXING** stage writing into the entity spine, link table, and FTS.
- **Cross-pillar bridge:** action-item → Task with `spawned_from` + evidence edges; meeting-as-note.
- **Model manager:** signed manifests, SHA-256, resumable downloads, disk preflight, offline import; hardware-tier auto-select.
- **NL fallback:** enable `app-nlp` LLM fallback now that a resident model exists.

### Phase 3 — Semantic Search, AI Workspace & Neighborhood Graph *(unify the brain; v1.0)*
- **Embeddings:** `embeddings` crate + EmbeddingGemma-300M/bge-base adapters; Matryoshka-256 + int8; incremental, content-hash-gated, debounced embedding; `embed_model` provenance per chunk.
- **Hybrid search:** FTS5 ∪ sqlite-vec KNN fused by Reciprocal Rank Fusion; typed filters (`type: tag: date: person: is:`) compiled to SQL; optional bge-reranker.
- **AI workspace:** `ai-workspace` retrieve→RRF→rerank→grounded-decode **AnswerV1**→**citation-verify**→refuse-with-`unanswered` pipeline; command palette **Ask** mode.
- **Suggestions:** reversible, cited auto-link/auto-tag `suggestion` rows generated as idle-time batch jobs; approval UI.
- **Graph:** neighborhood (not global) graph view over the link table.

### Phase 4 — Reach: Sync, OCR, Integrations *(optional / post-v1; v2.x)*
Activate the dormant `sync-core`: central blind relay, compress-then-encrypt (XChaCha20-Poly1305) over op ranges, Loro CRDT for note bodies, per-field LWW-by-HLC + OR-Set for structured entities. Plus OCR of image attachments, Parakeet-TDT "Turbo (English)" opt-in STT, Apple SpeechTranscriber native adapter, mobile companion, calendar/email integration, and a plugin SDK. **All of Phase 4 is explicitly a v1 non-goal;** it ships only if warranted, and only because the Phase 1 seam made it a re-encode rather than a re-model.

---

## 4. Delivery Workstreams

Workstreams run in parallel within and across phases; the table gives owner surface, the crates/modules touched, primary dependencies, and the earliest phase each becomes active.

| # | Workstream | Owns | Crates / Modules | Depends on | First active |
|---|-----------|------|------------------|-----------|--------------|
| W1 | **Foundation & Store** | SQLCipher, entity spine, op-log/HLC, NDJSON journals, migrations, key mgmt | `storage`, `sync-core` (dormant), `app-domain` | — | P1 |
| W2 | **Editor & Notes** | Tiptap nodes, `doc_json` validation, block projection, link extraction, MD I/O | `notes`, `ui/editor`, `ui/notebooks`, `ui/daily` | W1 | P1 |
| W3 | **Tasks / Reminders / Scheduler** | buckets, reorder, recurrence, dual-layer notifications, catch-up | `tasks`, `reminders`, `scheduler`, `links`, `ui/tasks` | W1 | P1 |
| W4 | **NL Entry** | grammar fast-path, highlighting, LLM fallback | `app-nlp` | W3; LLM (W7) for fallback | P1 (fast-path) |
| W5 | **Capture (per-OS)** | native audio adapters, ring buffers, RT discipline, capability report | `capture-api`, `capture-macos/windows/linux`, `media-pipeline` | W1 | P2 |
| W6 | **Speech / STT** | whisper.cpp two-pass; later Parakeet/Apple adapters | `speech-api`, `speech-whisper`, `speech-parakeet` (P4) | W5 | P2 |
| W7 | **Language / AI** | llama.cpp Qwen3, GBNF, MeetingArtifactV1, AnswerV1, suggestions | `llm-api`, `llm-llamacpp`, `ai-workspace` | W1; W5/W6 for meeting artifacts | P2 (artifacts) / P3 (workspace) |
| W8 | **Embeddings & Search** | FTS5, sqlite-vec, RRF, rerank, chunking, command palette | `search`, `embeddings`, `embeddings-gemma/bge`, `ui/command-palette`, `ui/search` | W1; W2 content; W7 rerank | P1 (FTS) / P3 (vec) |
| W9 | **Model Distribution** | signed manifests, checksums, resumable/offline download, disk preflight, tiers | `model-manager` | W1; network isolation | P2 |
| W10 | **Session Orchestration** | state machine, INDEXING-into-spine, recovery | `app-service` | W1, W5, W6, W7 | P2 |
| W11 | **Shell & Platform** | Tauri windows/tray, activation policy, capabilities, deep-link, global-shortcut | `tauri-app`, plugins | W1 | P1 |
| W12 | **Hardening & Release** | packaging, signing/notarize, updater, a11y, perf, recovery, docs | `export`, packaging, CI | all | continuous |

---

## 5. Milestones & Acceptance Gates

Each milestone has a functional gate and a measurable non-functional gate. Performance targets are inherited from the baseline and are **release-blocking** where marked.

| M | Milestone | Functional gate (must demonstrate) | Non-functional gate (blocking) |
|---|-----------|-----------------------------------|-------------------------------|
| **M0** | Walking skeleton | Store opens (SQLCipher, key in keystore); create a note; op appended to `entity_op`; rebuild all derived tables from the log; two Tauri windows + tray | Launch **< 2 s** cold; encrypted DB verified (no plaintext on disk) |
| **M1** | Notebook usable | Tiptap editor with all custom nodes; block projection + `[[wikilink]]`/`#tag`/backlinks; notebooks tree; daily note; quick-capture panel; MD import/export round-trips | Keystroke never lost across forced-kill; save→projection **< 50 ms** p95 |
| **M2** | Plan & remind | Task buckets (derived queries); start/deadline split; drag-reorder (fractional index); `rrule` recurrence `every`/`every!`; reminders fire via **both** scheduler layers; catch-up after forced-close; NL fast-path with highlighting | Reminder fires within **±1 s** of `fire_at` while running; **zero** missed-without-catch-up in 1000-reminder soak |
| **M3** | *Phase 1 ship (v0.1)* | FTS5 search + palette Go/Do; full offline; capability report honest per-OS | Memory **< 3 GB** typical; no telemetry; automatic crash recovery verified |
| **M4** | Local capture | All three OS adapters record app-audio + mic; exclude-self; never silent system-wide fallback; ring buffers, no RT-callback allocation | Capture start latency **< 100 ms**; raw PCM never crosses WebView (verified) |
| **M5** | Transcribe & understand | whisper two-pass; session state machine to COMPLETE; MeetingArtifactV1 with resolvable evidence for every fact; INDEXING writes spine + FTS | Live transcript latency **1–2 s**; artifact schema-valid or deterministic fallback (never malformed) |
| **M6** | *Phase 2 ship (v0.5)* | Action-item→Task with `spawned_from`+evidence; meeting-as-note; model manager signed/resumable/offline-import; NL LLM fallback | All model downloads checksum-verified; offline-import path works with network disabled |
| **M7** | Semantic + Ask | Incremental embeddings; sqlite-vec KNN; hybrid RRF + filters; **Ask** returns cited AnswerV1; **every citation resolves** or `unanswered:true` | FTS synchronous **< 10 ms**; **0%** unverified-citation display rate in eval set |
| **M8** | *Phase 3 ship (v1.0)* | Suggestion review (reversible, cited); neighborhood graph; end-to-end capture→note→task→ask demo | All M0–M7 gates re-pass in one build; signed/notarized installers on 3 OSes; a11y audit passed |

**Gate discipline:** a phase does not ship until every earlier gate re-passes in the shipping build (no regression carry-forward). Performance targets are measured on the defined reference hardware tiers, not developer laptops.

---

## 6. Testing Strategy

Testing is layered per subsystem; the local-first, crash-safe, evidence-carrying, capability-honest principles each get a dedicated verification lane. The `entity_op` log makes **rebuild-from-log** the master correctness oracle: any derived table (blocks, links, FTS, vectors) must be bit-reproducible from source.

| Area | What we verify | Method / harness | Gate |
|------|----------------|------------------|------|
| **Domain & state machine** | All session transitions incl. DEGRADED/FAILED/RECOVERING; no illegal transition; LLM never owns recording state | Exhaustive transition model tests + property-based fuzz over event sequences | M5, M8 |
| **Store & op-log seam** | Every mutation appends an op; derived tables **rebuild bit-identically** from `entity_op`; HLC monotonic; UUIDv7 unique | Golden-log replay; projection determinism diff; concurrency stress | M0, every phase |
| **Editor & CRDT-seam** | `doc_json` schema-valid before persist; `blockId` stable across edits; block projection & link extraction correct; MD I/O round-trips; doc re-encodable to Loro shape (seam smoke) | Snapshot tests on doc trees; fuzz random edits→project→assert; round-trip corpus | M1 |
| **Scheduler reliability & missed-reminder** | Fires within tolerance while running; OS layer B fires when app closed (macOS/Win); de-dup (one layer no-ops); catch-up coalesces past-due on launch/wake; snooze/edit cancels `os_handle`; Linux honestly reports no OS layer | Simulated-clock unit tests; forced-close + wall-clock integration; 1000-reminder soak; sleep/wake harness | M2 |
| **Tasks & recurrence** | Derived buckets match spec queries; start hides / deadline doesn't; fractional reorder stable under churn; `every` vs `every!` materialize-on-completion semantics | Query-equivalence tests; property tests on reorder key density; RRULE conformance vectors | M2 |
| **Capture adapters (per-OS)** | Correct app-audio isolation; exclude-self; never silent system-wide fallback; ring buffer no-overrun; **no allocation/DB/inference on RT callback**; capability report matches real device state | Per-OS device-loopback rigs; RT-callback allocation asserts; capability-matrix CI on each platform | M4 |
| **Media pipeline** | Monotonic clock; correct downmix/resample to 16 kHz; VAD/chunk/drift correctness | Signal-fixture golden outputs; drift-injection tests | M4 |
| **STT quality** | WER within budget per model profile; two-pass live→final convergence; timestamps accurate for evidence anchoring | Reference audio set + WER regression; timestamp-alignment tests | M5 |
| **LLM / artifact quality** | MeetingArtifactV1 & AnswerV1 always schema-valid (grammar + one repair + deterministic fallback); **no invented owners/dates**; every fact's evidence resolves | GBNF-constrained decode tests; hallucination-probe eval set; citation-resolution assertion | M5, M7 |
| **RAG / citation integrity** | Retrieval recall on eval queries; RRF fusion sane; **0% unverified citations displayed**; `unanswered` returned when unsupported | Labeled QA eval set; citation-verify unit gate; refusal-rate calibration | M7 |
| **Search** | FTS synchronous latency; filter→SQL compilation correctness; vector re-fuse improves ranking | Latency benchmarks; filter-parse unit tests; ranking eval | M3, M7 |
| **Offline** | Every core path works with **network fully disabled**; only `model-download`/`updater` ever open sockets; "Offline Ready" state correct | Network-namespace-isolated CI job; socket-audit assertion on core paths | Every phase |
| **Recovery / forced-kill** | `kill -9` mid-write loses no committed op; NDJSON journal replays; session recovers from any state; wake-from-sleep catch-up | Chaos harness (random SIGKILL); journal-replay fuzz; power/sleep simulation | M1, M2, M6 |
| **Accessibility** | Keyboard-only operation of editor, palette, tasks; screen-reader labels; focus order; reduced-motion; contrast | Automated a11y linters + manual AT (VoiceOver/NVDA/Orca) passes | M8 |
| **Release / signing** | macOS notarized DMG; Windows Authenticode MSI/EXE; Linux AppImage + Flatpak; updater signature-verifies; no telemetry in shipped binary | Per-OS packaging CI; signature verification; static telemetry-absence scan | M3, M6, M8 |
| **Performance** | Launch <2 s, capture <100 ms, live transcript 1–2 s, save <50 ms, FTS <10 ms, memory <3 GB | Automated perf harness on reference tiers; regression budgets in CI | Every ship gate |

---

## 7. Risks & Mitigations

| # | Risk | Impact | Likelihood | Mitigation |
|---|------|--------|-----------|-----------|
| R1 | Op-log seam under-designed → Phase 4 sync forces a re-model | High | Med | Build the seam in P1; run **rebuild-from-log** as a CI oracle every phase; keep note bodies block-IDed so Loro swap is a re-encode |
| R2 | JSON-doc-as-truth projection drift (blocks/links/FTS diverge from `doc_json`) | High | Med | Derived tables always rebuildable; projection determinism diff in CI; never dual-write bidirectionality |
| R3 | OS notification layer (B) unreliable / throttled on Windows/macOS; Linux has none | Med | High | Layer A (timer-wheel) is authoritative while running; catch-up sweep on launch/wake; **honest capability report**; de-dup so double-fire is impossible |
| R4 | Native capture regressions across OS updates (SCK/WASAPI/PipeWire API churn) | High | Med | Per-OS loopback CI rigs; capability-matrix gate; never silent system-wide fallback (fail loud) |
| R5 | LLM hallucinates owners/dates or unresolvable citations | High | Med | GBNF hard-constrain; "extract only from evidence" contract; citation-verify gate returns `unanswered` rather than display |
| R6 | Model size vs memory budget (<3 GB) on low tier | Med | Med | Hardware-tier auto-select (4B/8B/14B); Matryoshka-256 + int8 embeddings; single resident context + bounded queue |
| R7 | SQLCipher + sqlite-vec + FTS5 in one file: build/perf/extension-loading friction | Med | Med | Early spike (see O4); direct `rusqlite`, no `tauri-plugin-sql`; benchmark KNN at target corpus size |
| R8 | Forced-kill mid-transaction corrupts store | High | Low | WAL + append-only journals; `kill -9` chaos harness; committed-op durability assertion |
| R9 | Scope creep pulls Phase 4 (sync/OCR/plugins) into v1 | Med | High | Phase 4 items are explicit v1 non-goals in the Foundation; roadmap gate forbids new scope |
| R10 | Recurrence edge cases (DST, `after_completion`, timezones) mis-fire | Med | Med | Absolute `fire_at` UTC + IANA `tz`; RRULE conformance vectors; DST-boundary test suite |
| R11 | Tauri capability/CSP misconfig leaks SQL or FS to WebView | High | Low | Deny-by-default per-window capability files; WebView edits JSON only; IPC surface audit |

---

## 8. Open Decisions Requiring Prototype / Benchmark

These block their dependent workstreams and must be resolved by measurement, not opinion, before commit. Each names the decision, the experiment, and the phase-gate it blocks.

| # | Open decision | Prototype / benchmark to run | Blocks |
|---|---------------|------------------------------|--------|
| O1 | **Embedding model & truncation** — EmbeddingGemma-300M vs bge-base; 256-dim Matryoshka + int8 quality floor | Retrieval-quality eval (recall@k) on a mixed notes+transcript corpus at 256/384/768 dims; latency + RAM per tier | W8 / M7 |
| O2 | **Reranker cost/benefit** — bge-reranker on-device latency vs ranking gain | A/B RRF-only vs RRF+rerank on labeled QA set; p95 latency budget | W7/W8 / M7 |
| O3 | **STT profile matrix** — whisper base/small/medium live-vs-final split per tier; Parakeet turbo readiness | WER × latency × RAM grid on reference audio; convergence of two-pass | W6 / M5 |
| O4 | **sqlite-vec at scale inside SQLCipher** — KNN latency and index size at 100k+ chunks in an encrypted file | Synthetic-corpus benchmark; extension-load path under SQLCipher; backup/restore of single file | W1/W8 / M7 |
| O5 | **OS notification horizon** — the 14-day rolling registration window vs OS pending-notification limits (macOS 64-pending cap, Windows toast limits) | Measure per-OS pending caps; tune horizon + re-registration cadence; verify no silent drop | W3 / M2 |
| O6 | **Fractional-index scheme** — LexoRank vs plain rational; rebalance threshold under heavy reorder | Churn simulation measuring key-length growth and rebalance frequency | W3 / M2 |
| O7 | **LLM tier defaults** — Qwen3 4B/8B/14B artifact quality vs the <3 GB memory ceiling; MLX gains on Apple Silicon | MeetingArtifactV1 quality eval per tier; memory headroom with resident STT+embedder co-loaded | W7 / M5 |
| O8 | **NL-entry LLM-fallback trigger** — confidence threshold where grammar fast-path hands off | Precision/recall of the fast-path on a date/recurrence corpus; calibrate handoff so we "never invent a date" | W4 / M2–M6 |
| O9 | **Loro re-encode feasibility (seam validation)** — confirm the block-IDed doc maps cleanly to Loro movable-tree | Spike: encode a representative doc corpus into Loro and back; diff | W1 (validates R1) |

---

## 9. Summary

The delivery order is **notebook → meetings → brain → reach**, front-loading the untried bets (JSON-doc-as-truth, derived buckets, dual-layer scheduler, op-log seam) into Phase 1 while deferring the mature, inherited capture stack to Phase 2 and value-compounding AI to Phase 3. Twelve parallel workstreams converge on eight gated milestones, each with a blocking performance target measured on reference hardware. The test strategy makes **rebuild-from-log** the master correctness oracle and gives crash-safety, offline-capability, evidence-integrity, and capability-honesty each a dedicated lane. The single highest-leverage decision — the cheap op-log/HLC/UUID seam built now — is what keeps Phase 4 sync a re-encode rather than a re-model. Every open decision above is resolved by benchmark before its dependent workstream commits.
