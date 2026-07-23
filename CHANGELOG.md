# Changelog

All notable changes to Casual Note are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it reaches a shippable milestone.

## [Unreleased]

### Added
- Canonical design documentation set in `docs/` (PRD, Architecture, HLD, Data Model, Feature Specs, Roadmap, Research).
- Project governance: `README`, `LICENSE` (Apache-2.0) + `NOTICE`, `CONTRIBUTING`, `CODE_OF_CONDUCT`, `SECURITY`, `CLAUDE.md`,
  `SKILLS.md`, and the implementation `TRACKER.md`.
- Marketing site under `site/` (SEO-optimized, `llms.txt`, `robots.txt`, `sitemap.xml`, Open Graph), deployed to
  GitHub Pages at `notes.casualoffice.org`.
- CI/CD pipelines: multi-OS Rust build + clippy + tests, frontend typecheck/build, Tauri shell check, supply-chain
  audit (`cargo-deny`), telemetry-absence scan, GitHub Pages deploy, and a tag-triggered cross-platform release build.
- Phase 1 scaffold: a 26-crate Rust/Cargo workspace + Vite/React/TypeScript UI. Foundation crates implemented and
  unit-tested — `app-domain` (IDs/HLC/entities/events), `storage` (SQLCipher, migrations, `entity_op` op-log, NDJSON
  journal, content-addressed blobs, rebuild-from-log oracle), `notes`/`links` (doc-JSON projection, backlinks, Markdown
  I/O), `tasks` (derived buckets, fractional-index reorder), `reminders`/`scheduler` (rrule recurrence, dual-layer
  scheduler with honest Linux `RunningOnly`), `app-nlp` (grammar date parser), and `search` (FTS5/BM25). Core
  `cargo check`/`clippy -D warnings`/`fmt`/`test` and frontend `typecheck`/`lint`/`test`/`build` all green.

<!--
Milestone tags will appear here as they land:
  v0.1  M3  Phase 1 ship — Core Notebook + Plan + Store
  v0.5  M6  Phase 2 ship — Meeting Intelligence
  v1.0  M8  Phase 3 ship — Semantic brain + AI Workspace
-->
