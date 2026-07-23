/**
 * Typed bridge to the Rust core. The WebView talks to the core ONLY through
 * `invoke(cmd, args)` (HLD §6) and reconciles state from the `"app-event"` channel
 * (HLD §7). No SQL, no filesystem, no PCM ever crosses this boundary.
 *
 * HLD command names use dots (`notes.save`); Tauri command idents can't, so each is
 * invoked by its underscore form (`notes_save`). Argument keys are snake_case to
 * match the Rust command parameters exactly.
 *
 * Dev-mock fallback: when neither `window.__TAURI_INTERNALS__` nor `window.__TAURI__`
 * is present (i.e. `pnpm dev` in a plain browser, not inside Tauri) the same typed
 * surface is served from a realistic in-memory store so the app renders fully for
 * preview and screenshots. This branch is dead code inside a real Tauri window.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { mockCore } from "./mock";

export interface SaveResult {
  version: number;
  changed_block_ids: string[];
}

export interface NoteView {
  id: string;
  title: string | null;
  doc_json: string;
  notebook_id: string | null;
  daily_date: string | null;
  is_pinned: boolean;
  version: number;
  created_at: number;
  updated_at: number;
}

export interface NoteSummary {
  id: string;
  title: string | null;
  daily_date: string | null;
  is_pinned: boolean;
  updated_at: number;
}

export interface TaskView {
  id: string;
  title: string | null;
  project_id: string | null;
  area_id: string | null;
  notes_md: string | null;
  status: string;
  priority: number;
  someday: boolean;
  start_on: string | null;
  deadline_on: string | null;
  completed_at: number | null;
  order_key: string;
}

export interface ParsedEntry {
  kind: "note" | "task" | "reminder";
  title: string;
  start_on: string | null;
  deadline_on: string | null;
  priority: number;
  tags: string[];
  confidence: number;
}

export interface EntityRefT {
  kind: string;
  id: string;
}

export interface CaptureResult {
  entity_ref: EntityRefT;
  parsed: ParsedEntry;
}

export interface SearchHit {
  kind: string;
  id: string;
  title: string | null;
  snippet: string;
  bm25: number;
}

export interface SearchResults {
  query_id: string;
  hits: SearchHit[];
  complete: boolean;
}

/** A notebook/folder tree node (M1 — `notebooks.list`). Children nest recursively. */
export interface NotebookNode {
  id: string;
  name: string | null;
  parent_id: string | null;
  order_key: string;
  icon: string | null;
  color: string | null;
  children: NotebookNode[];
}

/** A resolved backlink into the open note (`links.backlinks`). */
export interface BacklinkRef {
  source_note_id: string;
  source_title: string | null;
  block_id: string | null;
  snippet: string;
}

/** A prose mention of the open note's title that is not (yet) a link. */
export interface UnlinkedMention {
  source_note_id: string;
  source_title: string | null;
  snippet: string;
}

/** The lean note shape returned by `daily.get_or_create` / `notes.import_markdown`. */
export interface Note {
  id: string;
  title: string | null;
  doc_json: string;
  version: number;
  created_at: number;
  updated_at: number;
}

export type Bucket = "Today" | "Upcoming" | "Anytime" | "Someday";

/** A sequenced AppEvent envelope (HLD §7). `type` discriminates the variant. */
export interface AppEventEnvelope {
  seq: number;
  type: string;
  [key: string]: unknown;
}

/**
 * True inside a real Tauri window. The globals are injected by the Tauri runtime
 * before any app code runs, so this is stable at module-eval time.
 */
export const isTauri: boolean =
  typeof window !== "undefined" &&
  ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);

/** Single dispatch point: real IPC inside Tauri, in-memory core otherwise. */
function call<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
  return isTauri ? invoke<T>(cmd, args) : mockCore.invoke<T>(cmd, args);
}

export const api = {
  /** `notes.create` → new note id. Title is derived server-side from the body. */
  notesCreate: (docJson?: string, notebookId?: string | null): Promise<string> =>
    call<string>("notes_create", {
      notebook_id: notebookId ?? null,
      daily_date: null,
      doc_json: docJson ?? null,
    }),

  notesGet: (noteId: string): Promise<NoteView> =>
    call<NoteView>("notes_get", { note_id: noteId }),

  notesSave: (noteId: string, docJson: string, baseVersion: number): Promise<SaveResult> =>
    call<SaveResult>("notes_save", {
      note_id: noteId,
      doc_json: docJson,
      base_version: baseVersion,
    }),

  notesList: (notebookId?: string | null): Promise<NoteSummary[]> =>
    call<NoteSummary[]>("notes_list", { notebook_id: notebookId ?? null }),

  // ---- M1: notebooks, daily note, backlinks, Markdown I/O ----------------

  /** `notebooks.list` → the notebook forest (roots with nested children). */
  notebooksList: (): Promise<NotebookNode[]> => call<NotebookNode[]>("notebooks_list", {}),

  /** `notebooks.create` → new notebook id. Omit `parentId` for a top-level notebook. */
  notebooksCreate: (name: string, parentId?: string | null): Promise<string> =>
    call<string>("notebooks_create", { name, parent_id: parentId ?? null }),

  /** `notes.move` → moves a note into a notebook (null/omit ⇒ top level). */
  notesMove: (noteId: string, notebookId: string | null): Promise<NoteView> =>
    call<NoteView>("notes_move", { note_id: noteId, notebook_id: notebookId }),

  /** `daily.get_or_create` → the note for a local `YYYY-MM-DD` date (idempotent). */
  dailyGetOrCreate: (date: string): Promise<Note> =>
    call<Note>("daily_get_or_create", { date }),

  /** `links.backlinks` → resolved inbound links for an entity (note/tag/person). */
  linksBacklinks: (entityId: string): Promise<BacklinkRef[]> =>
    call<BacklinkRef[]>("links_backlinks", { entity_id: entityId }),

  /** `links.unlinked_mentions` → prose mentions of the entity that are not links. */
  linksUnlinkedMentions: (entityId: string): Promise<UnlinkedMention[]> =>
    call<UnlinkedMention[]>("links_unlinked_mentions", { entity_id: entityId }),

  /** `notes.export_markdown` → CommonMark+GFM for a note. */
  notesExportMarkdown: (noteId: string): Promise<string> =>
    call<string>("notes_export_markdown", { note_id: noteId }),

  /** `notes.import_markdown` → a fresh note from Markdown (never overwrites). */
  notesImportMarkdown: (md: string, notebookId?: string | null): Promise<Note> =>
    call<Note>("notes_import_markdown", { md, notebook_id: notebookId ?? null }),

  tasksBucket: (bucket: Bucket): Promise<TaskView[]> =>
    call<TaskView[]>("tasks_bucket", { bucket }),

  tasksCreate: (title: string): Promise<TaskView> =>
    call<TaskView>("tasks_create", { input: { title } }),

  tasksComplete: (taskId: string): Promise<TaskView> =>
    call<TaskView>("tasks_complete", { task_id: taskId, at: null }),

  captureQuick: (text: string): Promise<CaptureResult> =>
    call<CaptureResult>("capture_quick", { text, kind_hint: null }),

  nlpParse: (text: string): Promise<ParsedEntry> => call<ParsedEntry>("nlp_parse", { text }),

  searchQuery: (q: string): Promise<SearchResults> =>
    call<SearchResults>("search_query", { q, mode: "go", limit: 20 }),
};

/**
 * The current Tauri window label (HLD §8.2). Outside Tauri — or if a `?window=`
 * override is present for browser preview — this resolves without touching the
 * runtime. Both windows load the same bundle, so the shell branches on this.
 */
export function currentWindowLabel(): string {
  if (typeof window !== "undefined") {
    const override = new URLSearchParams(window.location.search).get("window");
    if (override) return override;
  }
  if (!isTauri) return "main";
  try {
    return getCurrentWindow().label;
  } catch {
    return "main";
  }
}

/** Hide the current window (used by the frameless quick-capture surface). */
export async function hideCurrentWindow(): Promise<void> {
  if (!isTauri) return;
  try {
    await getCurrentWindow().hide();
  } catch {
    /* window plugin unavailable — no-op */
  }
}

/** Subscribe to the single core→WebView event channel (HLD §7). */
export function onAppEvent(handler: (ev: AppEventEnvelope) => void): Promise<UnlistenFn> {
  if (isTauri) {
    return listen<AppEventEnvelope>("app-event", (e) => handler(e.payload));
  }
  return Promise.resolve(mockCore.subscribe(handler));
}
