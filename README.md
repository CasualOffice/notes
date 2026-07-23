<div align="center">

<img src="./site/favicon.svg" width="76" alt="Casual Note" />

# Casual Note

### Casually a notebook. Quietly, the best local meeting assistant.

A fully-local, privacy-first desktop notebook that unifies **Notes · Reminders · Tasks · Meeting Intelligence**
into one encrypted store, one search index, one link graph, and one evidence-citing AI workspace — entirely on your device.

[![CI](https://github.com/CasualOffice/notes/actions/workflows/ci.yml/badge.svg)](https://github.com/CasualOffice/notes/actions/workflows/ci.yml)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](./LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-6b6357)](#platform-support)
[![Built with Rust](https://img.shields.io/badge/core-Rust-b45309)](https://www.rust-lang.org/)
[![Shell: Tauri 2](https://img.shields.io/badge/shell-Tauri%202-0f766e)](https://tauri.app/)

[Website](https://notes.casualoffice.org) · [Documentation](./docs) · [Roadmap](./docs/casual-note-roadmap.md) · [Security](./SECURITY.md) · [Contributing](./CONTRIBUTING.md)

</div>

---

## Overview

Casual Note is a serious tool for people who take a lot of notes and attend a lot of meetings — and who refuse to hand
their thinking to someone else's cloud. It is *casually a notebook* — notes, tasks, and reminders you reach for all day
— that is also a best-in-class local meeting recorder and understander.

Four pillars share **one encrypted local store, one search index, one link graph, and one AI workspace**. A task links
to the note that spawned it; a meeting *becomes* a note; a reminder can target any of them; and everything is findable
from a single command palette. There is no account, no cloud, and no telemetry. The network is touched only by two
named, user-consented services — model download and application updates — and every core workflow is verified to run
with the network switched off.

## Why Casual Note

- **Local-first, genuinely.** Your notes, tasks, audio, transcripts, and search index never leave the device. Offline
  is a tested, first-class mode — not a fallback.
- **Private by architecture, not by toggle.** Encrypted at rest (SQLCipher, key in the OS keychain), a hardened
  capability surface, and a CI gate that proves the shipped binary contains no telemetry.
- **Unified, not bundled.** One data model behind four pillars means your knowledge actually connects, instead of
  scattering across four disconnected apps.
- **Evidence or nothing.** The AI cites the transcript or note behind every fact it asserts — and returns "I don't
  know" rather than inventing an owner, a date, or a decision.
- **Built to last.** A small, fast Rust core behind a modern web UI. No Electron bloat, no background daemons, no
  surprises.

## Features

### Notes
A block and Markdown editor with a daily-note journal, `[[wikilinks]]`, `#tags`, backlinks, checklists, code blocks,
tables, and attachments. Blocks are the spine everything else attaches to.

### Reminders
Natural-language entry ("remind me every Monday at 9"), full recurrence, and a **dual-layer scheduler** that delivers
alerts whether the app is open or closed, catches up on anything missed after a restart, and is honest about
per-platform limits.

### Tasks
A focused planner in the spirit of the best task managers — **Today · Upcoming · Anytime · Someday** — with projects,
areas, scheduled/deadline dates, and drag-to-reorder. Meeting action items flow straight into tasks.

### Meeting Intelligence
Capture audio from a selected application and your microphone, transcribe **on-device** with Whisper, and generate
summaries, decisions, and action items with a **local LLM** — each fact linked to the transcript segment that supports
it.

### Unified Search & AI Workspace
Hybrid full-text (BM25) and semantic search across every pillar, a command palette to **Go** anywhere and **Do**
anything, and an AI workspace that answers questions about your knowledge base **with citations** — or honestly refuses.

## Architecture

Casual Note is a **Tauri 2 + Rust** desktop application with a **React/TypeScript** UI. A Cargo workspace of focused
crates owns all domain logic, storage, and orchestration; the WebView never sees SQL or the raw filesystem.

```
┌──────────────────────── Desktop WebView UI (React/TS, Tiptap) ─────────────────────────┐
│  editor · notebooks · daily · tasks · reminders · meetings · palette · search · ai      │
└───────────────────────────────────── Tauri command API ────────────────────────────────┘
                                              │
┌──────────────────────────────── Rust core (Tokio) ─────────────────────────────────────┐
│  app-service (orchestration, events)                                                     │
│  notes · tasks · reminders · scheduler · links · app-nlp · search · ai-workspace         │
│  capture · media-pipeline · speech (whisper.cpp) · llm (llama.cpp) · embeddings          │
│  storage (SQLite + SQLCipher, op-log, journals) · model-manager · export                 │
└──────────────────────────────────────────────────────────────────────────────────────┘
              Network permitted only for: model download · signed updates
```

| Layer | Technology |
|-------|-----------|
| Shell | Tauri 2, native WebView |
| Frontend | React, TypeScript (strict), Tiptap |
| Core | Rust (2021), Tokio |
| Store | SQLite + SQLCipher via `rusqlite`, NDJSON journals, append-only op-log |
| Search | FTS5/BM25 + `sqlite-vec` KNN fused with Reciprocal Rank Fusion |
| Speech | whisper.cpp (two-pass live + final) |
| Language | llama.cpp / GGUF (Qwen3 tiers), GBNF-constrained structured output |
| Embeddings | EmbeddingGemma-300M / bge (Matryoshka-256, int8) |

A design invariant worth calling out: every entity mutation appends to an **op-log** from which all derived tables
(blocks, links, search, vectors) can be rebuilt bit-for-bit. This is the master correctness oracle and the seam that
keeps optional future multi-device sync a re-encode rather than a re-model.

## Platform support

| Platform | Packaging | Notes |
|----------|-----------|-------|
| macOS | `.app` / DMG (universal), notarized | ScreenCaptureKit application-audio capture |
| Windows | MSI / EXE, Authenticode-signed | WASAPI process-loopback capture |
| Linux | AppImage + Flatpak | PipeWire capture; reminder scheduler runs while the app is open |

## Getting started (build from source)

**Prerequisites:** Rust ≥ 1.94, Node ≥ 20, and pnpm. Building the desktop shell on Linux additionally requires
`libwebkit2gtk-4.1-dev`, `libsoup-3.0-dev`, and `libayatana-appindicator3-dev`. The core crates build without them.

```bash
git clone git@github.com:CasualOffice/notes.git
cd notes

# Verify the offline core (no GUI system dependencies)
cargo check --workspace --exclude tauri-app
cargo test  --workspace --exclude tauri-app

# Frontend
cd ui && pnpm install && pnpm build && cd ..

# Run the full desktop app (requires platform GUI deps + downloaded models)
cargo tauri dev
```

**Before pushing** (and before tagging a release), run the full CI pipeline locally — it mirrors GitHub CI exactly, including the pinned supply-chain audit:

```bash
./scripts/ci-local.sh
```

Releases (signed installers for macOS, Windows, and Linux) are built by CI **only when a version tag is pushed** — never on a merge to `main`:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

## Documentation

The canonical design set lives in [`docs/`](./docs) (start with the [index](./docs/README.md)). All documents are
subordinate to the Design Foundation; for schema the **Data Model** is authoritative, for the interface surface the
**HLD**, and for scope the **PRD**.

| Document | Owns |
|----------|------|
| [Product Requirements](./docs/casual-note-prd.md) | Vision, personas, requirements, metrics, non-goals |
| [System Architecture](./docs/casual-note-architecture.md) | Decomposition, threading, security, packaging |
| [High-Level Design](./docs/casual-note-hld.md) | Command/event surface, adapter traits, sequences |
| [Data Model](./docs/casual-note-data-model.md) | The authoritative SQLite/SQLCipher schema |
| [Feature Specifications](./docs/casual-note-feature-specs.md) | Per-feature behavior & acceptance criteria |
| [Roadmap](./docs/casual-note-roadmap.md) | Sequencing, milestones, test strategy, risks |

Build progress is tracked in [`TRACKER.md`](./TRACKER.md); conventions and invariants for contributors are in
[`CLAUDE.md`](./CLAUDE.md).

## Roadmap

Casual Note is engineered notebook-first, so every pillar plugs into a real spine:

| Phase | Milestone | Focus |
|-------|-----------|-------|
| **1** | v0.1 | Core notebook, tasks, reminders, local store, full-text search |
| **2** | v0.5 | Meeting capture, transcription, and AI summaries |
| **3** | v1.0 | Semantic search, AI workspace, and knowledge graph |
| **4** | v2.x | Optional end-to-end-encrypted multi-device sync and integrations |

See the [full roadmap](./docs/casual-note-roadmap.md) for workstreams, acceptance gates, and the test strategy.
Casual Note is under active development toward the **v0.1 Core Notebook** milestone.

## Contributing

Contributions are welcome. Please read [CONTRIBUTING.md](./CONTRIBUTING.md) and [CLAUDE.md](./CLAUDE.md) first, and
[SECURITY.md](./SECURITY.md) before touching anything that reads user data. Participation is governed by our
[Code of Conduct](./CODE_OF_CONDUCT.md).

## Security

Security and privacy are the product. To report a vulnerability, follow the process in [SECURITY.md](./SECURITY.md) —
please do so privately and never include real user data.

## License

Licensed under the [Apache License 2.0](./LICENSE). Bundled inference engines and downloaded model weights carry their
own licenses; see [NOTICE](./NOTICE).

<div align="center"><sub>Own your notebook. Own your meetings.</sub></div>
