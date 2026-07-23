/**
 * In-memory dev-mock core (see `api.ts`). Serves the exact command surface and
 * event channel the Rust core would, so `pnpm dev` in a plain browser renders a
 * fully populated app for preview and screenshots. Never reached inside Tauri —
 * `api.call` only routes here when the Tauri globals are absent.
 *
 * Behavior mirrors the real projection where it matters: note titles are derived
 * from the first non-empty text node (parity with `app-service::derive_title`),
 * and mutations emit `NoteSaved` / `NoteProjected` / `TaskChanged` envelopes.
 */
import type { UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppEventEnvelope,
  CaptureResult,
  NoteSummary,
  NoteView,
  ParsedEntry,
  SaveResult,
  SearchResults,
  TaskView,
} from "./api";

type Handler = (ev: AppEventEnvelope) => void;

interface TiptapNode {
  type: string;
  text?: string;
  attrs?: Record<string, unknown>;
  content?: TiptapNode[];
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

/** Build a Tiptap `doc` from a heading + paragraphs. */
function doc(title: string, ...paras: string[]): string {
  const content: TiptapNode[] = [
    { type: "heading", attrs: { level: 1 }, content: [{ type: "text", text: title }] },
    ...paras.map((t) => ({ type: "paragraph", content: [{ type: "text", text: t }] })),
  ];
  return JSON.stringify({ type: "doc", content });
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

const DATE_RE = /\b(today|tonight|tomorrow|monday|tuesday|wednesday|thursday|friday|saturday|sunday|next week|\d{1,2}\s*(am|pm))\b/i;
const REMINDER_RE = /\b(remind|remember|ping|follow up|call|text)\b/i;
const TASK_RE = /\b(todo|task|buy|email|send|finish|review|draft|fix|write|ship|prepare|schedule|book)\b/i;

/** Cheap ParsedEntry approximation of `app-nlp` for offline preview. */
function parse(text: string): ParsedEntry {
  const tags = [...text.matchAll(/#([\w-]+)/g)].map((m) => m[1] ?? "").filter(Boolean);
  const bang = /!\s*([1-3])/.exec(text);
  const priority = bang?.[1] ? Number(bang[1]) : 0;
  const hasDate = DATE_RE.test(text);

  let kind: ParsedEntry["kind"] = "note";
  if (hasDate && REMINDER_RE.test(text)) kind = "reminder";
  else if (TASK_RE.test(text) || priority > 0 || hasDate) kind = "task";

  const title = text
    .replace(/#[\w-]+/g, "")
    .replace(/!\s*[1-3]/g, "")
    .trim()
    .slice(0, 120);

  return {
    kind,
    title: title || text.slice(0, 120),
    start_on: null,
    deadline_on: null,
    priority,
    tags,
    confidence: 0.72,
  };
}

class MockCore {
  private notes = new Map<string, NoteView>();
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

  private putNote(docJson: string, ageMs: number): NoteView {
    const id = uuid();
    const ts = this.now() - ageMs;
    const note: NoteView = {
      id,
      title: deriveTitle(docJson),
      doc_json: docJson,
      notebook_id: null,
      daily_date: null,
      is_pinned: false,
      version: 1,
      created_at: ts,
      updated_at: ts,
    };
    this.notes.set(id, note);
    return note;
  }

  private seed(): void {
    this.putNote(
      doc(
        "Product review — Q3 roadmap",
        "Three themes surfaced: capture friction, search recall, and the meeting-to-task handoff. Everyone agreed the op-log rebuild is the correctness backbone we lean on.",
        "Open question: how far do we push local inference before the first model download.",
      ),
      1000 * 60 * 26,
    );
    this.putNote(
      doc(
        "Reading — attention & note-taking",
        "The strongest recall comes from linking, not filing. Backlinks turn a flat pile of notes into a graph you can actually walk.",
      ),
      1000 * 60 * 60 * 5,
    );
    this.putNote(
      doc(
        "Weekly plan",
        "Ship the walking skeleton. Two windows, a tray, sub-two-second launch, and no plaintext on disk.",
        "Then: quick-capture routing and the backlinks panel.",
      ),
      1000 * 60 * 60 * 27,
    );
    // A genuinely empty note — deriveTitle yields null, rendered as "Untitled".
    this.putNote(EMPTY_DOC, 1000 * 60 * 60 * 50);

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

  private dispatch(cmd: string, args: Record<string, unknown>): unknown {
    switch (cmd) {
      case "notes_list": {
        return [...this.notes.values()]
          .sort((a, b) => b.updated_at - a.updated_at)
          .map<NoteSummary>((n) => ({
            id: n.id,
            title: n.title,
            daily_date: n.daily_date,
            is_pinned: n.is_pinned,
            updated_at: n.updated_at,
          }));
      }
      case "notes_get": {
        const note = this.notes.get(String(args["note_id"]));
        if (!note) throw new Error("note not found");
        return note;
      }
      case "notes_create": {
        const docJson = (args["doc_json"] as string | null) ?? EMPTY_DOC;
        const note = this.putNote(docJson, 0);
        this.emit("NoteSaved", {
          note_id: note.id,
          version: note.version,
          changed_block_ids: [],
        });
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
        this.emit("NoteSaved", {
          note_id: note.id,
          version: note.version,
          changed_block_ids: [note.id],
        });
        this.emit("NoteProjected", { note_id: note.id });
        return { version: note.version, changed_block_ids: [note.id] } satisfies SaveResult;
      }
      case "tasks_bucket": {
        return this.tasks.filter((t) => t.status === "open");
      }
      case "tasks_create": {
        const input = args["input"] as { title: string };
        const task = this.makeTask(input.title);
        this.tasks.push(task);
        this.emit("TaskChanged", { task_id: task.id });
        return task;
      }
      case "tasks_complete": {
        const task = this.tasks.find((t) => t.id === String(args["task_id"]));
        if (!task) throw new Error("task not found");
        task.status = "done";
        task.completed_at = this.now();
        this.emit("TaskChanged", { task_id: task.id });
        return task;
      }
      case "capture_quick": {
        const text = String(args["text"]);
        const parsed = parse(text);
        const ref = this.route(parsed, text);
        return { entity_ref: ref, parsed } satisfies CaptureResult;
      }
      case "nlp_parse": {
        return parse(String(args["text"]));
      }
      case "search_query": {
        const q = String(args["q"]).toLowerCase();
        const hits = [...this.notes.values()]
          .filter((n) => (n.title ?? "").toLowerCase().includes(q))
          .map((n) => ({
            kind: "note",
            id: n.id,
            title: n.title,
            snippet: n.title ?? "",
            bm25: 1,
          }));
        return { query_id: uuid(), hits, complete: true } satisfies SearchResults;
      }
      default:
        throw new Error(`mock: unhandled command ${cmd}`);
    }
  }

  private route(parsed: ParsedEntry, text: string): { kind: string; id: string } {
    if (parsed.kind === "task") {
      const task = this.makeTask(parsed.title, parsed.priority);
      this.tasks.push(task);
      this.emit("TaskChanged", { task_id: task.id });
      return { kind: "task", id: task.id };
    }
    if (parsed.kind === "reminder") {
      const id = uuid();
      this.reminders.add(id);
      this.emit("ReminderScheduled", { reminder_id: id });
      return { kind: "reminder", id };
    }
    const note = this.putNote(doc(parsed.title, text), 0);
    this.emit("NoteSaved", { note_id: note.id, version: 1, changed_block_ids: [] });
    this.emit("NoteProjected", { note_id: note.id });
    return { kind: "note", id: note.id };
  }
}

export const mockCore = new MockCore();
