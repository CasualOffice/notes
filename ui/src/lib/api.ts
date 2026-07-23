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
  notesCreate: (docJson?: string): Promise<string> =>
    call<string>("notes_create", {
      notebook_id: null,
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

  notesList: (): Promise<NoteSummary[]> =>
    call<NoteSummary[]>("notes_list", { notebook_id: null }),

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

/** Subscribe to the single core→WebView event channel (HLD §7). */
export function onAppEvent(handler: (ev: AppEventEnvelope) => void): Promise<UnlistenFn> {
  if (isTauri) {
    return listen<AppEventEnvelope>("app-event", (e) => handler(e.payload));
  }
  return Promise.resolve(mockCore.subscribe(handler));
}
