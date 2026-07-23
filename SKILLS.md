# SKILLS.md — Casual Note Skill Registry

A **skill** in Casual Note is a discrete, evidence-grounded capability the AI Workspace can perform over the user's
local knowledge (notes, tasks, reminders, meetings, transcripts). Skills run **entirely on-device** against the local
LLM (`llm-llamacpp`) and the retrieval stack (`search` + `embeddings` + `ai-workspace`). Every skill obeys the same
contract: **retrieve → ground → produce structured output → verify citations → refuse if unsupported.**

This file is the canonical registry. It also documents the *development* skills (Claude Code workflows) used to build
the project.

---

## 1. Skill contract (all product skills)

| Rule | Requirement |
|------|-------------|
| Grounded | Operates only over retrieved local evidence; never external APIs. |
| Cited | Every asserted fact carries resolvable `evidence_segment_ids` / entity+offset citations. |
| Honest | If evidence is insufficient, returns `unanswered: true` — it does **not** guess owners, dates, or facts. |
| Structured | Emits a versioned JSON schema (GBNF-constrained); one repair attempt, then deterministic fallback. |
| Reversible | Any mutation it proposes (auto-link, auto-tag, action→task) is a reviewable, undoable `suggestion` row. |
| Local & private | No network, no telemetry; runs within the memory/latency budgets in the PRD NFRs. |

## 2. Product skills (AI Workspace)

Grouped by phase in which they land (see `docs/casual-note-roadmap.md`).

### Phase 2 — Meeting understanding
| Skill | Input | Output (schema) |
|-------|-------|-----------------|
| `summarize.meeting` | session transcript | MeetingArtifactV1.executive_summary + topics |
| `extract.decisions` | transcript | decisions[] with evidence |
| `extract.action_items` | transcript | action_items[] (task, owner?, due?) with evidence |
| `extract.risks` / `extract.open_questions` | transcript | risks[] / open_questions[] with evidence |
| `bridge.action_to_task` | action_item | Task with `spawned_from` + evidence edges |

### Phase 3 — Unified brain
| Skill | Input | Output (schema) |
|-------|-------|-----------------|
| `ask.notes` | NL question + retrieval | **AnswerV1** (answer + citations, or `unanswered`) |
| `summarize.selection` | any entity/selection | grounded summary with citations |
| `suggest.links` | entity | reversible cited auto-link `suggestion` rows |
| `suggest.tags` | entity | reversible cited auto-tag `suggestion` rows |
| `transform.text` | selection + instruction | rewritten text (Markdown), evidence-preserving |

### Phase 1 — Natural-language entry (`app-nlp`, non-LLM fast path)
| Skill | Input | Output (schema) |
|-------|-------|-----------------|
| `parse.entry` | quick-capture string | **ParsedEntry** (route: note\|task\|reminder + parsed date/recurrence) |

> The grammar/regex fast path ships in Phase 1; the LLM fallback activates in Phase 2 once a resident model exists.
> The fallback's confidence threshold is calibrated so we **never invent a date** (open decision O8).

## 3. Adding a product skill

1. Define/extend the output JSON schema in the **Data Model** doc; add the GBNF grammar in `llm-api`.
2. Implement the retrieve→ground→decode→verify pipeline in `ai-workspace`; wire a Tauri command per the **HLD**.
3. Add a citation-resolution test and a hallucination-probe eval entry (see roadmap §6, gate M7).
4. Register the skill here and check the box in `TRACKER.md`.

## 4. Development skills (Claude Code)

How this repo is built with multi-agent workflows and Claude Code skills:

| Dev skill | When |
|-----------|------|
| `Workflow` (ultracode) | Multi-agent scaffolding, research, and fan-out implementation across crates. |
| `/code-review` | Review a working diff before commit. |
| `security-review` | Any change touching key management, capabilities, network isolation, or user data. |
| `simplify` | After a feature lands, sweep the diff for reuse/simplification. |
| `Explore` / `Plan` agents | Broad codebase search and implementation planning. |

**Convention:** substantial work is decomposed into a workflow (research → foundation → parallel authoring/impl →
consistency/verify review), and `TRACKER.md` is marked after each verified unit.
