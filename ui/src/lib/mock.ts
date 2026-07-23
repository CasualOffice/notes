/**
 * In-memory dev-mock core (see `api.ts`). Serves the exact command surface and
 * event channel the Rust core would, so `pnpm dev` in a plain browser renders a
 * fully populated app for preview and screenshots. Never reached inside Tauri —
 * `api.call` only routes here when the Tauri globals are absent.
 *
 * Behavior mirrors the real projection where it matters: note titles are derived
 * from the first non-empty text node (parity with `app-service::derive_title`),
 * wikilink marks project backlinks, and mutations emit `NoteSaved` /
 * `NoteProjected` / `BacklinksChanged` / `TaskChanged` envelopes. The M1 surface
 * (notebooks, daily note, backlinks, Markdown I/O) is seeded so every panel of the
 * app has something real to show.
 */
import type { UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppEventEnvelope,
  BacklinkRef,
  CaptureResult,
  Note,
  NotebookNode,
  NoteSummary,
  NoteView,
  ParsedEntry,
  SaveResult,
  SearchResults,
  TaskView,
  UnlinkedMention,
} from "./api";

type Handler = (ev: AppEventEnvelope) => void;

interface TiptapMark {
  type: string;
  attrs?: Record<string, unknown>;
}
interface TiptapNode {
  type: string;
  text?: string;
  attrs?: Record<string, unknown>;
  marks?: TiptapMark[];
  content?: TiptapNode[];
}

interface NotebookRow {
  id: string;
  name: string | null;
  parent_id: string | null;
  order_key: string;
  icon: string | null;
  color: string | null;
}

function uuid(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    return (c === "x" ? r : (r & 0x3) | 0x8).toString(16);
  });
}

/** A blank Tiptap document (single empty paragraph — no empty text nodes). */
const EMPTY_DOC = JSON.stringify({ type: "doc", content: [{ type: "paragraph" }] });

// ---- inline builders (parity with the marks the editor emits) -------------

function text(t: string): TiptapNode {
  return { type: "text", text: t };
}
function wikilink(target: string, targetId: string | null): TiptapNode {
  const attrs: Record<string, unknown> = { target };
  if (targetId) attrs["targetId"] = targetId;
  return { type: "text", text: target, marks: [{ type: "wikilink", attrs }] };
}
function tag(name: string): TiptapNode {
  return { type: "text", text: `#${name}`, marks: [{ type: "tag", attrs: { name } }] };
}
function mention(label: string): TiptapNode {
  return { type: "text", text: `@${label}`, marks: [{ type: "mention", attrs: { label } }] };
}
function para(...inline: TiptapNode[]): TiptapNode {
  return { type: "paragraph", content: inline };
}
function heading(t: string): TiptapNode {
  return { type: "heading", attrs: { level: 1 }, content: [text(t)] };
}
function task(checked: boolean, t: string): TiptapNode {
  return { type: "taskItem", attrs: { checked }, content: [text(t)] };
}

/** Depth-first text extraction, mirroring the Rust block projection. */
function extractText(node: TiptapNode): string[] {
  const out: string[] = [];
  const walk = (n: TiptapNode): void => {
    if (typeof n.text === "string") out.push(n.text);
    n.content?.forEach(walk);
  };
  walk(node);
  return out;
}

/** First non-empty text run, truncated to 120 chars (parity with `derive_title`). */
function deriveTitle(docJson: string): string | null {
  try {
    const parsed = JSON.parse(docJson) as TiptapNode;
    for (const t of extractText(parsed)) {
      const trimmed = t.trim();
      if (trimmed) return trimmed.slice(0, 120);
    }
  } catch {
    /* fall through */
  }
  return null;
}

/** First paragraph's flattened text, for a backlink snippet. */
function firstBodyText(docJson: string): string {
  try {
    const parsed = JSON.parse(docJson) as TiptapNode;
    for (const block of parsed.content ?? []) {
      if (block.type === "paragraph") {
        const t = extractText(block).join("").trim();
        if (t) return t.slice(0, 160);
      }
    }
  } catch {
    /* fall through */
  }
  return "";
}

/** Every wikilink mark in a doc, with its target title and optional target id. */
function collectWikilinks(docJson: string): { target: string; targetId: string | null }[] {
  const out: { target: string; targetId: string | null }[] = [];
  try {
    const parsed = JSON.parse(docJson) as TiptapNode;
    const walk = (n: TiptapNode): void => {
      for (const m of n.marks ?? []) {
        if (m.type === "wikilink") {
          const target = String(m.attrs?.["target"] ?? n.text ?? "");
          const tid = m.attrs?.["targetId"];
          out.push({ target, targetId: typeof tid === "string" ? tid : null });
        }
      }
      n.content?.forEach(walk);
    };
    walk(parsed);
  } catch {
    /* fall through */
  }
  return out;
}

const DATE_RE =
  /\b(today|tonight|tomorrow|monday|tuesday|wednesday|thursday|friday|saturday|sunday|next week|\d{1,2}\s*(am|pm))\b/i;
const REMINDER_RE = /\b(remind|remember|ping|follow up|call|text)\b/i;
const TASK_RE = /\b(todo|task|buy|email|send|finish|review|draft|fix|write|ship|prepare|schedule|book)\b/i;

/** Cheap ParsedEntry approximation of `app-nlp` for offline preview. */
function parse(text_: string): ParsedEntry {
  const tags = [...text_.matchAll(/#([\w-]+)/g)].map((m) => m[1] ?? "").filter(Boolean);
  const bang = /!\s*([1-3])/.exec(text_);
  const priority = bang?.[1] ? Number(bang[1]) : 0;
  const hasDate = DATE_RE.test(text_);

  let kind: ParsedEntry["kind"] = "note";
  if (hasDate && REMINDER_RE.test(text_)) kind = "reminder";
  else if (TASK_RE.test(text_) || priority > 0 || hasDate) kind = "task";

  const title = text_
    .replace(/#[\w-]+/g, "")
    .replace(/!\s*[1-3]/g, "")
    .trim()
    .slice(0, 120);

  return {
    kind,
    title: title || text_.slice(0, 120),
    start_on: null,
    deadline_on: null,
    priority,
    tags,
    confidence: 0.72,
  };
}

/** A minimal doc → Markdown pass for the preview export command. */
function docToMarkdown(docJson: string): string {
  const parsed = JSON.parse(docJson) as TiptapNode;
  const blocks: string[] = [];
  for (const b of parsed.content ?? []) {
    const inline = extractText(b).join("");
    switch (b.type) {
      case "heading": {
        const level = Number(b.attrs?.["level"] ?? 1);
        blocks.push(`${"#".repeat(level)} ${inline}`);
        break;
      }
      case "taskItem":
        blocks.push(`- [${b.attrs?.["checked"] ? "x" : " "}] ${inline}`);
        break;
      case "callout":
        blocks.push(`> [!${String(b.attrs?.["type"] ?? "note")}]\n> ${inline}`);
        break;
      default:
        if (inline) blocks.push(inline);
    }
  }
  return blocks.join("\n\n");
}

/** A minimal Markdown → doc pass for the preview import command. */
function markdownToDoc(md: string): string {
  const content: TiptapNode[] = [];
  for (const line of md.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const h = /^(#{1,6})\s+(.*)$/.exec(trimmed);
    if (h) {
      content.push({ type: "heading", attrs: { level: h[1]?.length ?? 1 }, content: [text(h[2] ?? "")] });
      continue;
    }
    const t = /^- \[([ xX])\]\s+(.*)$/.exec(trimmed);
    if (t) {
      content.push(task(t[1] !== " ", t[2] ?? ""));
      continue;
    }
    content.push(para(text(trimmed)));
  }
  if (content.length === 0) content.push({ type: "paragraph" });
  return JSON.stringify({ type: "doc", content });
}

class MockCore {
  private notes = new Map<string, NoteView>();
  private notebooks: NotebookRow[] = [];
  private tasks: TaskView[] = [];
  private reminders = new Set<string>();
  private handlers = new Set<Handler>();
  private seq = 0;

  constructor() {
    this.seed();
  }

  private now(): number {
    return Date.now();
  }

  private emit(type: string, extra: Record<string, unknown>): void {
    this.seq += 1;
    const ev: AppEventEnvelope = { seq: this.seq, type, ...extra };
    for (const h of this.handlers) h(ev);
  }

  private putNote(
    docJson: string,
    ageMs: number,
    opts: { notebookId?: string | null; dailyDate?: string | null; id?: string } = {},
  ): NoteView {
    const id = opts.id ?? uuid();
    const ts = this.now() - ageMs;
    const note: NoteView = {
      id,
      title: deriveTitle(docJson),
      doc_json: docJson,
      notebook_id: opts.notebookId ?? null,
      daily_date: opts.dailyDate ?? null,
      is_pinned: false,
      version: 1,
      created_at: ts,
      updated_at: ts,
    };
    this.notes.set(id, note);
    return note;
  }

  private notebook(name: string, parentId: string | null, order: string): string {
    const id = uuid();
    this.notebooks.push({
      id,
      name,
      parent_id: parentId,
      order_key: order,
      icon: null,
      color: null,
    });
    return id;
  }

  private seed(): void {
    const work = this.notebook("Work", null, "a0");
    const personal = this.notebook("Personal", null, "a1");
    const research = this.notebook("Research", work, "a0");

    // Pre-mint ids so notes can link to one another by target id.
    const roadmapId = uuid();
    const readingId = uuid();
    const planId = uuid();

    this.putNote(
      JSON.stringify({
        type: "doc",
        content: [
          heading("Product review — Q3 roadmap"),
          para(
            text("Three themes surfaced: capture friction, search recall, and the "),
            wikilink("meeting-to-task handoff", null),
            text("."),
          ),
          para(
            text("Grounding for the "),
            tag("roadmap"),
            text(" came straight out of "),
            wikilink("Reading — attention & note-taking", readingId),
            text(" — thanks "),
            mention("sam"),
            text("."),
          ),
          task(false, "Draft the Q3 acceptance checklist"),
          task(true, "Confirm the op-log rebuild is the correctness backbone"),
          {
            type: "callout",
            attrs: { type: "info" },
            content: [para(text("Open question: how far do we push local inference before the first model download."))],
          },
        ],
      }),
      1000 * 60 * 26,
      { notebookId: work, id: roadmapId },
    );

    this.putNote(
      JSON.stringify({
        type: "doc",
        content: [
          heading("Reading — attention & note-taking"),
          para(
            text("The strongest recall comes from linking, not filing. Backlinks turn a flat pile of notes into a graph you can actually walk."),
          ),
          para(text("This directly shaped the "), wikilink("Product review — Q3 roadmap", roadmapId), text(".")),
        ],
      }),
      1000 * 60 * 60 * 5,
      { notebookId: research, id: readingId },
    );

    this.putNote(
      JSON.stringify({
        type: "doc",
        content: [
          heading("Weekly plan"),
          para(text("Ship the walking skeleton. Two windows, a tray, sub-two-second launch, and no plaintext on disk.")),
          para(text("Then: quick-capture routing and the backlinks panel for "), wikilink("Product review — Q3 roadmap", roadmapId), text(".")),
        ],
      }),
      1000 * 60 * 60 * 27,
      { notebookId: personal, id: planId },
    );

    // A genuinely empty note — deriveTitle yields null, rendered as "Untitled".
    this.putNote(EMPTY_DOC, 1000 * 60 * 60 * 50, { notebookId: personal });

    this.tasks = [
      this.makeTask("Draft the M0 acceptance checklist", 2),
      this.makeTask("Confirm keystore fallback on Linux", 1),
      this.makeTask("Wire the tray menu actions"),
    ];
  }

  private makeTask(title: string, priority = 0): TaskView {
    return {
      id: uuid(),
      title,
      project_id: null,
      area_id: null,
      notes_md: null,
      status: "open",
      priority,
      someday: false,
      start_on: null,
      deadline_on: null,
      completed_at: null,
      order_key: String(this.tasks.length + 1).padStart(4, "0"),
    };
  }

  subscribe(handler: Handler): UnlistenFn {
    this.handlers.add(handler);
    return () => {
      this.handlers.delete(handler);
    };
  }

  invoke<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
    return new Promise((resolve, reject) => {
      // A small delay keeps async ordering realistic for the UI.
      setTimeout(() => {
        try {
          resolve(this.dispatch(cmd, args) as T);
        } catch (e) {
          reject(e instanceof Error ? e : new Error(String(e)));
        }
      }, 40);
    });
  }

  private summary(n: NoteView): NoteSummary {
    return {
      id: n.id,
      title: n.title,
      daily_date: n.daily_date,
      is_pinned: n.is_pinned,
      updated_at: n.updated_at,
    };
  }

  private assembleTree(): NotebookNode[] {
    const build = (parentId: string | null): NotebookNode[] =>
      this.notebooks
        .filter((nb) => nb.parent_id === parentId)
        .sort((a, b) => a.order_key.localeCompare(b.order_key))
        .map((nb) => ({ ...nb, children: build(nb.id) }));
    return build(null);
  }

  private backlinksFor(entityId: string): BacklinkRef[] {
    const target = this.notes.get(entityId);
    const title = target?.title ?? "";
    const out: BacklinkRef[] = [];
    for (const n of this.notes.values()) {
      if (n.id === entityId) continue;
      const links = collectWikilinks(n.doc_json);
      const hit = links.some((l) => l.targetId === entityId || (title && l.target === title));
      if (hit) {
        out.push({
          source_note_id: n.id,
          source_title: n.title,
          block_id: null,
          snippet: firstBodyText(n.doc_json),
        });
      }
    }
    return out;
  }

  private unlinkedMentionsFor(entityId: string): UnlinkedMention[] {
    const target = this.notes.get(entityId);
    const title = target?.title;
    if (!title) return [];
    const out: UnlinkedMention[] = [];
    for (const n of this.notes.values()) {
      if (n.id === entityId) continue;
      const links = collectWikilinks(n.doc_json);
      const alreadyLinked = links.some((l) => l.targetId === entityId || l.target === title);
      if (alreadyLinked) continue;
      const body = extractText(JSON.parse(n.doc_json) as TiptapNode).join(" ");
      if (body.includes(title)) {
        out.push({ source_note_id: n.id, source_title: n.title, snippet: firstBodyText(n.doc_json) });
      }
    }
    return out;
  }

  private dispatch(cmd: string, args: Record<string, unknown>): unknown {
    switch (cmd) {
      case "notes_list": {
        const nb = args["notebook_id"] as string | null;
        return [...this.notes.values()]
          .filter((n) => (nb == null ? true : n.notebook_id === nb))
          .sort((a, b) => b.updated_at - a.updated_at)
          .map((n) => this.summary(n));
      }
      case "notes_get": {
        const note = this.notes.get(String(args["note_id"]));
        if (!note) throw new Error("note not found");
        return note;
      }
      case "notes_create": {
        const docJson = (args["doc_json"] as string | null) ?? EMPTY_DOC;
        const notebookId = (args["notebook_id"] as string | null) ?? null;
        const note = this.putNote(docJson, 0, { notebookId });
        this.emit("NoteSaved", { note_id: note.id, version: note.version, changed_block_ids: [] });
        this.emit("NoteProjected", { note_id: note.id });
        return note.id;
      }
      case "notes_save": {
        const note = this.notes.get(String(args["note_id"]));
        if (!note) throw new Error("note not found");
        const docJson = String(args["doc_json"]);
        note.doc_json = docJson;
        note.title = deriveTitle(docJson);
        note.version += 1;
        note.updated_at = this.now();
        this.emit("NoteSaved", { note_id: note.id, version: note.version, changed_block_ids: [note.id] });
        this.emit("NoteProjected", { note_id: note.id });
        this.emit("BacklinksChanged", { note_id: note.id });
        return { version: note.version, changed_block_ids: [note.id] } satisfies SaveResult;
      }
      case "notebooks_list":
        return this.assembleTree();
      case "notebooks_create": {
        const name = String(args["name"]);
        const parentId = (args["parent_id"] as string | null) ?? null;
        const siblings = this.notebooks.filter((nb) => nb.parent_id === parentId).length;
        const id = this.notebook(name, parentId, `a${siblings}`);
        this.emit("NotebooksChanged", {});
        return id;
      }
      case "notes_move": {
        const note = this.notes.get(String(args["note_id"]));
        if (!note) throw new Error("note not found");
        note.notebook_id = (args["notebook_id"] as string | null) ?? null;
        note.updated_at = this.now();
        this.emit("NoteProjected", { note_id: note.id });
        return note;
      }
      case "daily_get_or_create": {
        const date = String(args["date"]);
        const existing = [...this.notes.values()].find((n) => n.daily_date === date);
        const note =
          existing ??
          this.putNote(JSON.stringify({ type: "doc", content: [heading(date)] }), 0, {
            dailyDate: date,
          });
        if (!existing) {
          this.emit("NoteSaved", { note_id: note.id, version: note.version, changed_block_ids: [] });
          this.emit("NoteProjected", { note_id: note.id });
        }
        return {
          id: note.id,
          title: note.title,
          doc_json: note.doc_json,
          version: note.version,
          created_at: note.created_at,
          updated_at: note.updated_at,
        } satisfies Note;
      }
      case "links_backlinks":
        return this.backlinksFor(String(args["entity_id"]));
      case "links_unlinked_mentions":
        return this.unlinkedMentionsFor(String(args["entity_id"]));
      case "notes_export_markdown": {
        const note = this.notes.get(String(args["note_id"]));
        if (!note) throw new Error("note not found");
        return docToMarkdown(note.doc_json);
      }
      case "notes_import_markdown": {
        const md = String(args["md"]);
        const notebookId = (args["notebook_id"] as string | null) ?? null;
        const note = this.putNote(markdownToDoc(md), 0, { notebookId });
        this.emit("NoteSaved", { note_id: note.id, version: note.version, changed_block_ids: [] });
        this.emit("NoteProjected", { note_id: note.id });
        return {
          id: note.id,
          title: note.title,
          doc_json: note.doc_json,
          version: note.version,
          created_at: note.created_at,
          updated_at: note.updated_at,
        } satisfies Note;
      }
      case "tasks_bucket":
        return this.tasks.filter((t) => t.status === "open");
      case "tasks_create": {
        const input = args["input"] as { title: string };
        const t = this.makeTask(input.title);
        this.tasks.push(t);
        this.emit("TaskChanged", { task_id: t.id });
        return t;
      }
      case "tasks_complete": {
        const t = this.tasks.find((x) => x.id === String(args["task_id"]));
        if (!t) throw new Error("task not found");
        t.status = "done";
        t.completed_at = this.now();
        this.emit("TaskChanged", { task_id: t.id });
        return t;
      }
      case "capture_quick": {
        const text_ = String(args["text"]);
        const parsed = parse(text_);
        const ref = this.route(parsed, text_);
        return { entity_ref: ref, parsed } satisfies CaptureResult;
      }
      case "nlp_parse":
        return parse(String(args["text"]));
      case "search_query": {
        const q = String(args["q"]).toLowerCase();
        const hits = [...this.notes.values()]
          .filter((n) => (n.title ?? "").toLowerCase().includes(q))
          .map((n) => ({ kind: "note", id: n.id, title: n.title, snippet: n.title ?? "", bm25: 1 }));
        return { query_id: uuid(), hits, complete: true } satisfies SearchResults;
      }
      default:
        throw new Error(`mock: unhandled command ${cmd}`);
    }
  }

  private route(parsed: ParsedEntry, raw: string): { kind: string; id: string } {
    if (parsed.kind === "task") {
      const t = this.makeTask(parsed.title, parsed.priority);
      this.tasks.push(t);
      this.emit("TaskChanged", { task_id: t.id });
      return { kind: "task", id: t.id };
    }
    if (parsed.kind === "reminder") {
      const id = uuid();
      this.reminders.add(id);
      this.emit("ReminderScheduled", { reminder_id: id });
      return { kind: "reminder", id };
    }
    const note = this.putNote(
      JSON.stringify({ type: "doc", content: [heading(parsed.title), para(text(raw))] }),
      0,
    );
    this.emit("NoteSaved", { note_id: note.id, version: 1, changed_block_ids: [] });
    this.emit("NoteProjected", { note_id: note.id });
    return { kind: "note", id: note.id };
  }
}

export const mockCore = new MockCore();
