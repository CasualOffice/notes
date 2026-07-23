# Security Policy

Casual Note's core promise is that **your data never leaves your device**. Security and privacy are features, not
settings. This policy covers how we handle vulnerabilities and the invariants that protect users.

## Reporting a vulnerability

Please report security issues **privately** — do not open a public issue for anything exploitable.

- Email the maintainers with a description, reproduction steps, affected version/OS, and impact.
- Allow reasonable time for a fix before public disclosure. We aim to acknowledge within a few days.
- **Never include real user data** (notes, transcripts, audio) in a report — redact or synthesize.

## Security invariants (what we guarantee)

These are enforced in code and tested (see `docs/casual-note-architecture.md` §Security and the roadmap test matrix):

1. **Local-only data.** Audio, notes, tasks, transcripts, and the search index never leave the device. Only
   `model-download` and `updater` may open a socket, and only with explicit user consent. Core paths are tested with
   the network disabled.
2. **Encryption at rest.** The store is SQLite + SQLCipher; the DB key lives in the OS keystore (Keychain / Credential
   Manager / Secret Service), never in plaintext on disk.
3. **WebView isolation.** The WebView never receives SQL or raw filesystem access. All DB/FS access is Rust-side; Tauri
   capabilities are deny-by-default per window; strict CSP; no remote content.
4. **Untrusted model files.** Models are verified against signed manifests + SHA-256; parsers are size-bounded and
   hardened; no arbitrary code execution from model repositories.
5. **No telemetry by default.** The shipped binary is scanned for telemetry absence in CI. Any future analytics must be
   opt-in, local, and independently disableable.
6. **Least privilege & supply chain.** Pinned dependencies, signed/notarized releases, and a deny-by-default capability
   surface. Path traversal and export abuse are mitigated via canonical paths and user-mediated save dialogs.

## Scope

In scope: the desktop application, its Rust core, the Tauri shell, capability configuration, key management, model
verification, and the update path. Out of scope for v1: multi-device sync, cloud services, and third-party integrations
(these are explicit v1 non-goals).

## Handling secrets in development

Never commit real keys, model weights, or user data. `.gitignore` excludes `*.db`, `/models`, `*.gguf`, `.env`, and
`/appdata`. Use `.env.example` for config templates.
