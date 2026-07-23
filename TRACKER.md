# Casual Note — Implementation Tracker

Live build status. **Checked the moment a unit is complete _and_ verified.** Legend: `[ ]` todo · `[~]` in progress ·
`[x]` done · `[!]` blocked. Milestones (M0–M8) and workstreams (W1–W12) reference
[`docs/casual-note-roadmap.md`](./docs/casual-note-roadmap.md).

_Last updated: 2026-07-23 — **M0 + M1 complete** (notebook usable) and **Phase-2 engine foundations** landed; full `./scripts/ci-local.sh` green. Next: M2 (meeting session pipeline)._

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
- [x] `git init` + first structure commit + pushed to `github.com:CasualOffice/notes`
- [x] Marketing site (`site/`): SEO meta + JSON-LD, `llms.txt`, `robots.txt`, `sitemap.xml`, OG/favicon, `CNAME` → `notes.casualoffice.org`
- [x] CI/CD pipelines (`.github/workflows/`: `ci.yml`, `pages.yml`, `release.yml`) + `deny.toml` supply-chain policy

---

## Phase 1 — Core Notebook, Planning & Local Store  → ship v0.1

### Scaffold (walking skeleton spine)
- [x] Cargo workspace root (`Cargo.toml` with all 26 members + shared workspace deps; license Apache-2.0)
- [x] `rust-toolchain.toml`, `rustfmt.toml`, `.editorconfig`, CI (`.github/workflows`), `deny.toml`
- [x] All 26 crate directories with `Cargo.toml` + `//!`-documented `lib.rs` (Phase-1 crates implemented; later-phase stubbed)
- [x] `ui/` frontend scaffold (Vite + React + TS strict + Tiptap + feature-module folders) — typecheck/lint/test/build green
- [x] `tauri-app` crate: `tauri.conf.json`, `main.rs`, `build.rs`, deny-by-default capabilities
- [x] Workspace `cargo check --workspace --exclude tauri-app` + `clippy -D warnings` + `fmt` + full `cargo test` **green**

### W1 — Foundation & Store (`storage`, `app-domain`, `sync-core` dormant)
- [x] `app-domain`: UUIDv7/ULID IDs, HLC clock, entity kinds, enums, error taxonomy, `AppEvent`
- [x] `storage`: `rusqlite` + SQLCipher (`bundled-sqlcipher-vendored-openssl`); open/keyed connection; OS keystore key mgmt
- [x] Migrations: universal `entity` spine + per-type detail tables + polymorphic `link` table (per Data Model)
- [x] `entity_op` append-only op-log (UUIDv7/ULID + HLC) + NDJSON entity-mutation journal
- [x] Single logical DB writer (WAL, busy_timeout/retry, delta-safe detail upsert); content-addressed blob store
- [x] **Rebuild-from-log oracle**: derived tables reproduce bit-identically from `entity_op` (test passes)

### W2 — Editor & Notes (`notes`, `ui/editor`, `ui/notebooks`, `ui/daily`)
- [x] `notes`: Tiptap `doc_json` parse + schema validation before persist
- [x] Block-index projection + link extraction (`[[wiki]]` / `#tag` / `@mention`) → blocks/links tables
- [x] Markdown import/export round-trip
- [x] UI: Tiptap editor with `blockId`-stamped custom nodes (todo/callout/code/table/lists) + slash menu; `[[wikilink]]` autocomplete, `#tag`, `@mention`; debounced autosave; backlinks panel
- [x] UI: notebooks/folder tree; daily-note (`daily.get_or_create`) Today entry
- [x] UI: quick-capture window view (`capture.quick`) + global hotkey registered

### W3 — Tasks / Reminders / Scheduler (`tasks`, `reminders`, `scheduler`, `links`, `ui/tasks`)
- [x] `tasks`: areas/projects/headings/tasks/checklists; `start_on`/`deadline_on` split
- [x] Derived Today / Upcoming / Anytime / Someday buckets (query-equivalence tested)
- [x] Fractional-index drag-reorder (stable under churn)
- [x] `reminders`: first-class polymorphic reminders + reminder state machine
- [x] Recurrence via `rrule` (`every` / `every!`, materialize-on-completion)
- [x] `scheduler`: Layer A Tokio timer-wheel (min-heap on `fire_at`), rebuilt from SQLite on boot
- [~] `scheduler`: Layer B OS one-shot handoff — Linux honest `RunningOnly` done; macOS/Windows adapters stubbed
- [x] Missed-reminder catch-up sweep on launch/wake; de-dup (no double-fire, no drop)

### W4 — NL Entry (`app-nlp`)
- [x] Grammar/regex fast path → `ParsedEntry` (route note|task|reminder + date/recurrence); never invents a date
- [~] Live highlighting in quick-capture _(highlight spans produced; UI wiring pending)_ — LLM fallback deferred to Phase 2

### W8 (P1 slice) — Search (`search`, `ui/command-palette`, `ui/search`)
- [x] FTS5/BM25 over the entity spine (contentless FTS + rowid↔entity map)
- [~] Command palette **Go** + **Do** modes _(query models + Do registry done; UI pending)_

### Milestone gates
_**M0 and M1 are DONE**, verified green via `./scripts/ci-local.sh`. Phase-2 engine foundations (capture/speech/llm contracts, media-pipeline DSP, model-manager) also landed. **Next: M2** — wire the session state machine (capture → STT → LLM → MeetingArtifactV1 → action-items-to-tasks)._
- [x] **M0** Walking skeleton: store opens (SQLCipher, key in keystore); create note → op appended to `entity_op` → rebuild-from-log bit-identical → note text is NOT plaintext in the DB file (encryption verified); two windows + tray + global hotkey; full workspace + Tauri shell compile — **verified via `ci-local.sh`**
- [x] **M1** Notebook usable: Tiptap editor (custom nodes + slash menu) + block projection + `[[wikilinks]]`/backlinks; notebooks tree; daily note; quick-capture window; Markdown round-trip; **keystroke-never-lost-on-kill and save→projection p95 < 50 ms encoded as passing tests** — verified via `ci-local.sh`
- [ ] **M2** Plan & remind: buckets; start/deadline; reorder; recurrence; reminders fire via both layers; catch-up; ±1 s fire; 0 missed in 1000-reminder soak
- [ ] **M3** *Phase 1 ship (v0.1)*: FTS5 + palette; full offline; honest capability report; memory < 3 GB; crash recovery verified

---

## Phase 2 — Meeting Intelligence, Summaries & Text Search  → ship v0.5
- [~] W5 Capture: `capture-api` trait + capability/DTO contracts done; **`media-pipeline` DSP done** (downmix, 16 kHz resample, VAD, chunking, ring buffer — tested); native OS adapters (SCK/WASAPI/PipeWire FFI) pending
- [~] W6 STT: `speech-api` trait + segment/hypothesis/profile types done; whisper.cpp FFI adapter pending
- [~] W7 LLM & artifacts: `llm-api` trait + **MeetingArtifactV1 / AnswerV1 types** done; llama.cpp FFI + GBNF-constrained decode + repair→fallback pending
- [ ] W10 Session state machine `NEW→…→COMPLETE` (+DEGRADED/FAILED/RECOVERING); INDEXING writes spine+FTS
- [ ] Cross-pillar bridge: action-item → Task (`spawned_from` + evidence); meeting-as-note
- [x] W9 Model manager: signed manifests, SHA-256 verify, disk preflight, offline import, hardware-tier select (resumable HTTP downloader behind a trait; real network impl deferred)
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
- [x] CI: fmt + clippy + test + Tauri-shell build + `cargo-deny` audit + telemetry scan — **all verified green locally** (macOS/Windows matrix non-blocking until platform adapters land)
- [x] Release pipeline: cross-platform installers, **tag-only** (`v*`) — never runs on merges
- [ ] Packaging: macOS DMG/notarize · Windows MSI Authenticode · Linux AppImage + Flatpak
- [ ] Accessibility audit (keyboard-only, screen readers, contrast, reduced-motion)
- [ ] Performance harness on reference tiers (launch/capture/transcript/save/FTS/memory budgets)

---

## Open decisions (resolve by benchmark before dependent work — roadmap §8)
- [ ] O1 embedding model & 256/int8 quality floor · [ ] O2 reranker cost/benefit · [ ] O3 STT profile matrix
- [ ] O4 sqlite-vec at scale in SQLCipher · [ ] O5 OS notification horizon vs caps · [ ] O6 fractional-index scheme
- [ ] O7 LLM tier defaults vs <3 GB · [ ] O8 NL LLM-fallback threshold · [ ] O9 Loro re-encode feasibility spike
