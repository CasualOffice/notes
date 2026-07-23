/**
 * Typed bridge to the Rust core. The WebView talks to the core ONLY through
 * `invoke(cmd, args)` (HLD §6) and reconciles state from the `"app-event"` channel
 * (HLD §7). No SQL, no filesystem, no PCM ever crosses this boundary.
 *
 * HLD command names use dots (`notes.save`); Tauri command idents can't, so each is
 * invoked by its underscore form (`notes_save`). Argument keys are snake_case to
 * match the Rust command parameters exactly.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

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

export const api = {
  notesCreate: (docJson?: string): Promise<string> =>
    invoke<string>("notes_create", {
      notebook_id: null,
      daily_date: null,
      doc_json: docJson ?? null,
    }),

  notesGet: (noteId: string): Promise<NoteView> =>
    invoke<NoteView>("notes_get", { note_id: noteId }),

  notesSave: (noteId: string, docJson: string, baseVersion: number): Promise<SaveResult> =>
    invoke<SaveResult>("notes_save", {
      note_id: noteId,
      doc_json: docJson,
      base_version: baseVersion,
    }),

  notesList: (): Promise<NoteSummary[]> =>
    invoke<NoteSummary[]>("notes_list", { notebook_id: null }),

  tasksBucket: (bucket: Bucket): Promise<TaskView[]> =>
    invoke<TaskView[]>("tasks_bucket", { bucket }),

  tasksCreate: (title: string): Promise<TaskView> =>
    invoke<TaskView>("tasks_create", { input: { title } }),

  tasksComplete: (taskId: string): Promise<TaskView> =>
    invoke<TaskView>("tasks_complete", { task_id: taskId, at: null }),

  captureQuick: (text: string): Promise<CaptureResult> =>
    invoke<CaptureResult>("capture_quick", { text, kind_hint: null }),

  nlpParse: (text: string): Promise<ParsedEntry> =>
    invoke<ParsedEntry>("nlp_parse", { text }),

  searchQuery: (q: string): Promise<SearchResults> =>
    invoke<SearchResults>("search_query", { q, mode: "go", limit: 20 }),
};

/** A sequenced AppEvent envelope (HLD §7). `type` discriminates the variant. */
export interface AppEventEnvelope {
  seq: number;
  type: string;
  [key: string]: unknown;
}

/** Subscribe to the single core→WebView event channel. */
export function onAppEvent(handler: (ev: AppEventEnvelope) => void): Promise<UnlistenFn> {
  return listen<AppEventEnvelope>("app-event", (e) => handler(e.payload));
}
