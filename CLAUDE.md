# CLAUDE.md — Conventions for building Casual Note

This file orients AI agents and human contributors. Read it before writing code. It is deliberately short; the
**canonical truth is in `docs/`**.

## What this project is

Casual Note is a **fully-local, privacy-first desktop notebook** (Tauri 2 + React/TS shell, Rust core) unifying four
pillars — **Notes, Reminders, Tasks, Meeting Intelligence** — over one encrypted SQLite/SQLCipher store, one link
graph, one search index, and one evidence-citing AI workspace. It is offline-first: only `model-download` and
`updater` may ever open a socket, and only with explicit user consent.

## Document authority (do not violate)

All code must conform to the design docs. When they conflict, this precedence holds:

1. **Design Foundation** (spine) → product scope & principles.
2. **Data Model** (`docs/casual-note-data-model.md`) → *authoritative* for every table, column, key, constraint, and
   JSON schema (MeetingArtifactV1 / AnswerV1 / ParsedEntry). Never invent a field; cite this doc.
3. **HLD** (`docs/casual-note-hld.md`) → *authoritative* for the Tauri command surface, the `AppEvent` model, and OS
   adapter traits.
4. **Architecture** (`docs/casual-note-architecture.md`) → decomposition, threading, security, packaging.
5. **PRD** (`docs/casual-note-prd.md`) → requirements (FR-*/NFR-*) and non-goals.
6. **Feature Specs** (`docs/casual-note-feature-specs.md`) → per-feature behavior & acceptance criteria.

If reality forces a change to a contract, **change the doc first**, then the code.

## Non-negotiable invariants

- **Local-first / offline.** Core crates build and pass tests with the network disabled. No cloud, no accounts, no
  third-party AI APIs, no telemetry by default.
- **The WebView never sees SQL or the raw filesystem.** All DB access is Rust-side via **direct `rusqlite`** (SQLCipher),
  never `tauri-plugin-sql`. Capabilities are deny-by-default per window.
- **Op-log seam.** Every entity mutation appends to `entity_op` (UUIDv7/ULID + HLC). Derived tables (blocks, links,
  FTS, vectors) must be **bit-reproducibly rebuildable from the log** — this is the master correctness oracle. Never
  dual-write bidirectionally.
- **Crash-safe.** Writes go through the single logical DB writer (SQLite WAL) + NDJSON journals; a `kill -9` must lose
  no committed op. Recovery replays the journal.
- **The LLM never owns recording state.** Capture/transcription continue if generation fails.
- **Evidence or nothing.** AI facts carry resolvable `evidence_segment_ids`/citations; unsupported answers return
  `unanswered:true` rather than display. Never invent owners or dates.
- **Capability honesty.** Platform limits are reported, not hidden (e.g. Linux reminder scheduler is `RunningOnly`;
  capture never silently falls back to system-wide audio).
- **No RT-callback sin.** No allocation, DB, inference, or logging on an OS real-time audio callback.

## Workspace layout

```
crates/
  app-domain      app-service     tauri-app
  notes  tasks  reminders  scheduler  links  app-nlp
  embeddings  ai-workspace  search
  capture-api  capture-macos  capture-windows  capture-linux  media-pipeline
  speech-api  speech-whisper  speech-parakeet
  llm-api  llm-llamacpp
  storage  model-manager  export  sync-core(dormant)
ui/               React/TS feature modules (editor, tasks, reminders, meetings, palette, search, ai, graph)
```

## Build & check commands

```bash
cargo check --workspace --exclude tauri-app   # core crates, no GUI system deps
cargo test  --workspace --exclude tauri-app   # unit + property tests
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
cd ui && pnpm install && pnpm typecheck && pnpm build
cargo tauri dev                               # full app (needs webkit2gtk-4.1 on Linux + models)
```

## Coding conventions

- Rust 2021, `rustfmt` default, `clippy -D warnings` clean. Errors: `thiserror` in libs, typed & retryable per the
  Architecture error taxonomy; no `unwrap()` in non-test code paths that can fail at runtime.
- IDs are UUIDv7/ULID; timestamps are absolute UTC with IANA tz where user-facing; recurrence uses the `rrule` crate.
- `storage` uses `rusqlite` with `bundled-sqlcipher-vendored-openssl` (compiles from source; no system libs).
- Frontend: TypeScript strict, Tiptap for the editor with `blockId`-stamped custom nodes; `doc_json` is the source of
  truth, projected to blocks/links Rust-side.
- Every new crate: a `//!` module doc linking the owning doc section, plus `#![forbid(unsafe_code)]` unless a native
  adapter genuinely needs FFI (capture-*, speech-*, llm-* may).

## Working rhythm

- Keep `TRACKER.md` current: check the box the moment a unit of work is complete and verified.
- Prefer small, reviewable, compiling increments. A change that doesn't `cargo check` is not done.
- When unsure about a contract, grep `docs/` before guessing.
