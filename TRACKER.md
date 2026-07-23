# Casual Note — Implementation Tracker

Live build status. **Checked the moment a unit is complete _and_ verified.** Legend: `[ ]` todo · `[~]` in progress ·
`[x]` done · `[!]` blocked. Milestones (M0–M8) and workstreams (W1–W12) reference
[`docs/casual-note-roadmap.md`](./docs/casual-note-roadmap.md).

_Last updated: 2026-07-23 — Phase 0 (governance + scaffold) underway._

---

## Phase 0 — Governance & Documentation

- [x] Canonical design docs authored (`docs/`: PRD, Architecture, HLD, Data Model, Feature Specs, Roadmap, Research)
- [x] Docs consistency audit + index (`docs/README.md`)
- [x] `LICENSE` (Apache-2.0) + `NOTICE`
- [x] `README.md` (root)
- [x] `CONTRIBUTING.md`
- [x] `CODE_OF_CONDUCT.md`
- [x] `SECURITY.md`
- [x] `CHANGELOG.md`
- [x] `CLAUDE.md` (conventions + doc authority + invariants)
- [x] `SKILLS.md` (AI skill registry + dev skills)
- [x] `TRACKER.md` (this file)
- [x] `.gitignore`
- [x] `git init` + first structure commit

---

## Phase 1 — Core Notebook, Planning & Local Store  → ship v0.1

### Scaffold (walking skeleton spine)
- [ ] Cargo workspace root (`Cargo.toml` with all members + shared workspace deps)
- [ ] `rust-toolchain.toml`, `rustfmt.toml`, `clippy`/`.editorconfig`, CI stub (`.github/workflows`)
- [ ] All crate directories created with `Cargo.toml` + `//!`-documented `lib.rs` stubs
- [ ] `ui/` frontend scaffold (Vite + React + TS strict + Tiptap dep + feature-module folders)
- [ ] `tauri-app` crate: `tauri.conf.json`, `main.rs`, `build.rs`, deny-by-default capabilities
- [ ] Workspace parses (`cargo metadata`) and core crates `cargo check` clean

### W1 — Foundation & Store (`storage`, `app-domain`, `sync-core` dormant)
- [ ] `app-domain`: UUIDv7/ULID IDs, HLC clock, entity kinds, enums, error taxonomy, `AppEvent`
- [ ] `storage`: `rusqlite` + SQLCipher (`bundled-sqlcipher-vendored-openssl`); open/keyed connection; OS keystore key mgmt
- [ ] Migrations: universal `entity` spine + per-type detail tables + polymorphic `link` table (per Data Model)
- [ ] `entity_op` append-only op-log (UUIDv7/ULID + HLC) + NDJSON entity-mutation journal
- [ ] Single logical DB writer (WAL, busy_timeout/retry); content-addressed blob store
- [ ] **Rebuild-from-log oracle**: derived tables reproduce bit-identically from `entity_op` (CI test)

### W2 — Editor & Notes (`notes`, `ui/editor`, `ui/notebooks`, `ui/daily`)
- [ ] `notes`: Tiptap `doc_json` parse + schema validation before persist
- [ ] Block-index projection + link extraction (`[[wiki]]` / `#tag` / `@mention`) → blocks/links tables
- [ ] Markdown import/export round-trip
- [ ] UI: Tiptap editor with `blockId`-stamped custom nodes (todo, callout, code, table, wikilink, tag, mention)
- [ ] UI: notebooks/folder tree; daily-note spine keyed by `daily_date`
- [ ] UI: quick-capture frameless panel + global hotkey

### W3 — Tasks / Reminders / Scheduler (`tasks`, `reminders`, `scheduler`, `links`, `ui/tasks`)
- [ ] `tasks`: areas/projects/headings/tasks/checklists; `start_on`/`deadline_on` split
- [ ] Derived Today / Upcoming / Anytime / Someday buckets (query-equivalence tested)
- [ ] Fractional-index drag-reorder (stable under churn)
- [ ] `reminders`: first-class polymorphic reminders + reminder state machine
- [ ] Recurrence via `rrule` (`every` / `every!`, materialize-on-completion)
- [ ] `scheduler`: Layer A Tokio timer-wheel (min-heap on `fire_at`), rebuilt from SQLite on boot
- [ ] `scheduler`: Layer B OS one-shot handoff (macOS/Windows), honest `RunningOnly` on Linux
- [ ] Missed-reminder catch-up sweep on launch/wake; de-dup (no double-fire, no drop)

### W4 — NL Entry (`app-nlp`)
- [ ] Grammar/regex fast path → `ParsedEntry` (route note|task|reminder + date/recurrence)
- [ ] Live highlighting in quick-capture (LLM fallback deferred to Phase 2)

### W8 (P1 slice) — Search (`search`, `ui/command-palette`, `ui/search`)
- [ ] FTS5/BM25 over the entity spine (contentless FTS + rowid↔entity map)
- [ ] Command palette **Go** + **Do** modes

### Milestone gates
- [ ] **M0** Walking skeleton: store opens (SQLCipher, key in keystore); create note; op appended; rebuild from log; 2 windows + tray; launch < 2 s; no plaintext on disk
- [ ] **M1** Notebook usable: editor + projection + wikilinks/backlinks; notebooks; daily; quick-capture; MD round-trip; keystroke never lost on kill; save→projection < 50 ms p95
- [ ] **M2** Plan & remind: buckets; start/deadline; reorder; recurrence; reminders fire via both layers; catch-up; ±1 s fire; 0 missed in 1000-reminder soak
- [ ] **M3** *Phase 1 ship (v0.1)*: FTS5 + palette; full offline; honest capability report; memory < 3 GB; crash recovery verified

---

## Phase 2 — Meeting Intelligence, Summaries & Text Search  → ship v0.5
- [ ] W5 Capture: carry forward macOS/Windows/Linux adapters behind unified trait; ring buffers; RT discipline
- [ ] W6 STT: whisper.cpp two-pass (live base / final small-medium)
- [ ] W7 LLM & artifacts: llama.cpp/GGUF Qwen3; GBNF-constrained MeetingArtifactV1; repair→fallback; evidence IDs
- [ ] W10 Session state machine `NEW→…→COMPLETE` (+DEGRADED/FAILED/RECOVERING); INDEXING writes spine+FTS
- [ ] Cross-pillar bridge: action-item → Task (`spawned_from` + evidence); meeting-as-note
- [ ] W9 Model manager: signed manifests, SHA-256, resumable download, disk preflight, offline import, tier auto-select
- [ ] W4 NL LLM fallback enabled (resident model exists)
- [ ] **M4** Local capture · **M5** Transcribe & understand · **M6** *Phase 2 ship (v0.5)*

---

## Phase 3 — Semantic Search, AI Workspace & Neighborhood Graph  → ship v1.0
- [ ] W8 Embeddings: `embeddings` crate + gemma/bge adapters; Matryoshka-256 + int8; incremental, content-hash-gated
- [ ] W8 Hybrid search: FTS5 ∪ sqlite-vec KNN fused by RRF; typed filters → SQL; optional bge-reranker
- [ ] W7 AI workspace: retrieve→RRF→rerank→grounded-decode **AnswerV1**→citation-verify→refuse; palette **Ask** mode
- [ ] Suggestions: reversible, cited auto-link/auto-tag `suggestion` rows + approval UI
- [ ] Neighborhood graph view over the link table
- [ ] **M7** Semantic + Ask · **M8** *Phase 3 ship (v1.0)*

---

## Phase 4 — Reach (optional / post-v1; explicit v1 non-goals)
- [ ] Activate `sync-core` (E2E sync, Loro CRDT) · OCR · Parakeet Turbo · Apple SpeechTranscriber · integrations · plugin SDK

---

## Cross-cutting (W12 — continuous)
- [ ] CI: fmt + clippy + test + offline-network-isolation job + telemetry-absence scan
- [ ] Packaging: macOS DMG/notarize · Windows MSI Authenticode · Linux AppImage + Flatpak
- [ ] Accessibility audit (keyboard-only, screen readers, contrast, reduced-motion)
- [ ] Performance harness on reference tiers (launch/capture/transcript/save/FTS/memory budgets)

---

## Open decisions (resolve by benchmark before dependent work — roadmap §8)
- [ ] O1 embedding model & 256/int8 quality floor · [ ] O2 reranker cost/benefit · [ ] O3 STT profile matrix
- [ ] O4 sqlite-vec at scale in SQLCipher · [ ] O5 OS notification horizon vs caps · [ ] O6 fractional-index scheme
- [ ] O7 LLM tier defaults vs <3 GB · [ ] O8 NL LLM-fallback threshold · [ ] O9 Loro re-encode feasibility spike
