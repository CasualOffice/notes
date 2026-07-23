# Casual Note — Documentation

**Casual Note** is a fully-local, privacy-first desktop notebook that unifies four pillars — **Notes, Reminders, Tasks, and Meeting Intelligence** — into one encrypted local store, one search index, one link graph, and one evidence-citing AI workspace. It is *casually a notebook* that also happens to be a best-in-class local meeting recorder/understander (inherited from the prior EchoNote product). Everything runs on-device; the network is touched only by two named, user-consented services (`model-download`, `updater`).

This directory holds the canonical design set. All documents are subordinate to the **Design Foundation** (the canonical spine): where any document disagrees with the Foundation, the Foundation wins and that document is corrected.

## Documents

| Document | What it owns |
|----------|-------------|
| [casual-note-prd.md](./casual-note-prd.md) | **Product Requirements** — vision, positioning, personas & JTBD, the four pillars, the full functional-requirement catalog (FR-N/R/T/M/S/A/P), non-functional requirements (NFR), success metrics, and v1 non-goals. The *what* and *why* for humans. |
| [casual-note-architecture.md](./casual-note-architecture.md) | **System Architecture** — crate/component decomposition, process & thread model, editor↔Rust sync contract, cross-platform capture, the dual-layer notification scheduler, storage & encryption, network isolation, the extended security threat model, reliability, and packaging. *How the pieces fit and why.* |
| [casual-note-hld.md](./casual-note-hld.md) | **High-Level Design** — runtime designs of the hard subsystems: deployment view, the public Tauri command surface, the `AppEvent` model, OS adapter traits, sequence diagrams for the five load-bearing flows, and the NFR envelope with acceptance gates. *Wire-level interfaces and control flow.* |
| [casual-note-data-model.md](./casual-note-data-model.md) | **Data Model & Storage Schema** — the authoritative SQLite (SQLCipher) physical schema: entity spine, per-type detail tables, the polymorphic link graph, chunks/embeddings, journals, on-disk layout, migrations, and the MeetingArtifactV1 / AnswerV1 / ParsedEntry JSON contracts. *Columns, keys, constraints.* |
| [casual-note-feature-specs.md](./casual-note-feature-specs.md) | **Feature Specifications** — behavior-level specs (preconditions, behavior, states, edge cases, testable acceptance criteria) for every user-facing surface: editor, quick capture, tasks, reminders, meetings, AI workspace, unified search/palette, export, and capability honesty. *Per-feature behavior.* |
| [casual-note-roadmap.md](./casual-note-roadmap.md) | **Roadmap, Delivery Plan & Test Strategy** — notebook-first sequencing rationale, the four-phase roadmap, twelve delivery workstreams, milestones M0–M8 with acceptance gates, the layered test strategy, the risk register, and open decisions requiring prototype/benchmark. *Sequencing and how we know it's done.* |
| [casual-note-research.md](./casual-note-research.md) | **Deep Research Dossier & Competitive Analysis** — the web-grounded research synthesis (2025–2026) across six domains that feeds every decision above: document models, task/reminder semantics, local-first sync/CRDT/encryption, on-device AI, the Tauri shell, the unified knowledge model, and a competitive matrix. *Background and evidence — schema/interface sketches here are illustrative, not canonical.* |

## How these fit together (reading order)

1. **Start with the [PRD](./casual-note-prd.md)** to understand the product — the four pillars, who it's for, and what v1 must and must not do.
2. **Read the [Research dossier](./casual-note-research.md)** if you want the *why behind the how* — it grounds every architectural choice in what leading products and libraries actually do. (It precedes the rest chronologically; its inline code is exploratory and defers to the Data Model where they differ.)
3. **Then the [Architecture](./casual-note-architecture.md)** for the system decomposition and the trust/correctness boundaries, followed by the **[HLD](./casual-note-hld.md)** for the runtime designs, command/event contracts, and sequence flows.
4. **Consult the [Data Model](./casual-note-data-model.md)** as the authoritative schema reference — it is the source of truth for entity/field names and JSON contracts that the other documents cite.
5. **Use the [Feature Specs](./casual-note-feature-specs.md)** for exact per-feature behavior and acceptance criteria when building or testing a surface.
6. **Track delivery with the [Roadmap](./casual-note-roadmap.md)** — sequencing, milestones, gates, risks, and the benchmarks that must resolve open decisions before dependent work commits.

### Ownership boundaries (who's authoritative for what)

- **Product scope, personas, requirement IDs, metrics** → PRD
- **Decomposition, threading, security, packaging** → Architecture
- **Runtime subsystem designs, Tauri command/event surface, adapter traits** → HLD
- **Physical schema, columns, JSON schemas** → Data Model
- **Per-feature behavior and acceptance criteria** → Feature Specs
- **Sequencing and acceptance gates** → Roadmap
- **Background evidence** → Research (non-normative)

When two documents appear to disagree on a schema detail, the **Data Model** is authoritative for schema, the **HLD** for the command/interface surface, and the **PRD/Foundation** for scope.
