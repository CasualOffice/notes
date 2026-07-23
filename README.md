<h1 align="center">Casual Note</h1>

<p align="center"><em>Casually a notebook. Quietly, the best local meeting assistant.</em></p>

<p align="center">
  A fully-local, privacy-first desktop notebook that unifies <strong>Notes · Reminders · Tasks · Meeting Intelligence</strong>
  into one encrypted store, one search index, one link graph, and one evidence-citing AI workspace — all on-device.
</p>

---

## What it is

Casual Note is a warm, everyday notebook — notes, tasks, and reminders — that *also* happens to be a best-in-class
local meeting recorder and understander. Four pillars share **one encrypted local store, one search index, one link
graph, and one AI workspace**:

- **📝 Notes** — a block/Markdown editor with daily notes, `[[wikilinks]]`, `#tags`, backlinks, checklists, and attachments.
- **⏰ Reminders** — natural-language entry, recurrence, and a dual-layer notification scheduler that survives app close.
- **✅ Tasks** — Things-style *Today / Upcoming / Anytime / Someday*, projects & areas, drag-reorder.
- **🎙️ Meeting Intelligence** — capture app + mic audio, transcribe locally, and generate summaries, decisions, and
  action items with a local LLM — with every fact linked to transcript evidence. Action items flow straight into tasks.

Everything runs on the device. The network is touched only by two named, user-consented services:
`model-download` and `updater`. There is no account, no cloud, and no telemetry by default.

## Status

🚧 **Pre-alpha — Phase 1 (Core Notebook) in progress.** See [`TRACKER.md`](./TRACKER.md) for live build status and
[`docs/casual-note-roadmap.md`](./docs/casual-note-roadmap.md) for the full plan.

## Documentation

The canonical design set lives in [`docs/`](./docs/). Start with the [documentation index](./docs/README.md).

| Doc | Owns |
|-----|------|
| [PRD](./docs/casual-note-prd.md) | Product scope, personas, requirements, metrics |
| [Architecture](./docs/casual-note-architecture.md) | Decomposition, threading, security, packaging |
| [HLD](./docs/casual-note-hld.md) | Command/event surface, adapter traits, sequences |
| [Data Model](./docs/casual-note-data-model.md) | The authoritative SQLite/SQLCipher schema |
| [Feature Specs](./docs/casual-note-feature-specs.md) | Per-feature behavior & acceptance criteria |
| [Roadmap](./docs/casual-note-roadmap.md) | Sequencing, milestones, test strategy, risks |
| [Research](./docs/casual-note-research.md) | Web-grounded background (non-normative) |

> **Doc authority:** all documents are subordinate to the Design Foundation. For schema, the **Data Model** wins; for
> the command/interface surface, the **HLD** wins; for scope, the **PRD** wins.

## Tech stack

| Layer | Choice |
|-------|--------|
| Shell | **Tauri 2** + native WebView |
| Frontend | **React + TypeScript** + Tiptap editor |
| Core | **Rust** (Tokio), Cargo workspace of focused crates |
| Store | **SQLite + SQLCipher** via `rusqlite` (encrypted, single file) + NDJSON journals |
| Search | **FTS5/BM25** + `sqlite-vec` KNN fused with Reciprocal Rank Fusion |
| Speech | **whisper.cpp** (two-pass live + final) |
| LLM | **llama.cpp / GGUF** (Qwen3 tiers), GBNF-constrained structured output |
| Embeddings | EmbeddingGemma-300M / bge (Matryoshka-256 + int8) |

## Repository layout

```
casualnote/
├─ crates/            # Rust workspace (domain, storage, notes, tasks, reminders, scheduler, …)
├─ ui/                # React/TypeScript frontend (Tiptap editor, feature modules)
├─ docs/              # Canonical design documents (PRD, Architecture, HLD, Data Model, …)
├─ CLAUDE.md          # Conventions & build guide for AI/human contributors
├─ SKILLS.md          # AI Workspace skill registry + dev skills
├─ TRACKER.md         # Live implementation tracker
└─ …                  # LICENSE, CONTRIBUTING, SECURITY, CODE_OF_CONDUCT
```

## Building (developer preview)

> Requires Rust ≥ 1.94, Node ≥ 20, and pnpm. On Linux, the Tauri shell also needs `webkit2gtk-4.1` and
> `libayatana-appindicator3` dev packages. The non-GUI core crates build without those.

```bash
# Core Rust crates (no system GUI deps)
cargo check --workspace --exclude tauri-app

# Frontend
cd ui && pnpm install && pnpm build

# Full app (needs platform GUI deps + models)
cargo tauri dev
```

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md). Please also read [CLAUDE.md](./CLAUDE.md) for architecture conventions and
the doc-authority hierarchy, and [SECURITY.md](./SECURITY.md) before touching anything that reads user data.

## License

[Apache-2.0](./LICENSE) © 2026 Casual Note contributors. Bundled inference engines (whisper.cpp, llama.cpp) and models
carry their own licenses (see [NOTICE](./NOTICE)).
