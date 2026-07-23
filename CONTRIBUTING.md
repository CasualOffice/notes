# Contributing to Casual Note

Thanks for helping build a genuinely private, local-first notebook. This guide covers how to propose changes and the
bar every change must clear.

## Before you start

- Read [`CLAUDE.md`](./CLAUDE.md) — conventions, invariants, and the **document-authority hierarchy**.
- Skim the doc that owns the area you're touching (see the table in `CLAUDE.md`). The `docs/` set is canonical; code
  conforms to it, not the reverse. If you must change a contract, **change the doc in the same PR**.
- Check [`TRACKER.md`](./TRACKER.md) to see what's in flight and claim an unchecked item.

## Development setup

Requires **Rust ≥ 1.94**, **Node ≥ 20**, and **pnpm**. On Linux, the Tauri shell additionally needs
`webkit2gtk-4.1` and `libayatana-appindicator3` dev packages (the core crates build without them).

```bash
git clone <repo> && cd casualnote
cargo check --workspace --exclude tauri-app
cd ui && pnpm install
```

## The bar for a change

A change is "done" only when all of these pass:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test  --workspace --exclude tauri-app
cd ui && pnpm typecheck && pnpm lint && pnpm test
```

Plus, where relevant:

- **Op-log oracle** — if you added a mutation, it appends to `entity_op` and derived tables rebuild bit-identically
  from the log. Add/extend a rebuild-from-log test.
- **Offline** — core paths must pass with the network disabled. Only `model-download`/`updater` may open a socket.
- **Crash-safety** — mutating paths survive `kill -9` with no committed-op loss.
- **Evidence** — AI output carries resolvable citations or returns `unanswered`.
- **Capability honesty** — no silent fallback that hides a platform limitation.

## Commit & PR conventions

- **Conventional Commits**: `feat(tasks): …`, `fix(scheduler): …`, `docs(hld): …`, `chore: …`, `test: …`.
- Keep PRs small and single-purpose; a PR that doesn't `cargo check` will not be reviewed.
- Reference the requirement/milestone it advances (e.g. "advances M2 / FR-T-03") and tick `TRACKER.md`.
- Run `/code-review` on your diff; run `security-review` for anything touching keys, capabilities, network, or user data.

## Reporting bugs & proposing features

- **Bugs**: include OS + version, repro steps, and whether it reproduces offline. Never paste real note/transcript
  content — redact.
- **Features**: check they aren't a v1 non-goal (see PRD/Architecture §non-goals — e.g. sync, mobile, plugins, cloud AI
  are explicitly out for v1). Open a discussion referencing the PRD before large work.

## Code of conduct

Participation is governed by [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md).

## License

By contributing, you agree your contributions are licensed under the [Apache License 2.0](./LICENSE).
