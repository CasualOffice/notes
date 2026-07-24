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

/** A project row projection (`tasks.projects_areas`). */
export interface ProjectView {
  id: string;
  name: string | null;
  area_id: string | null;
  status: string;
  order_key: string;
}

/** An area row projection (`tasks.projects_areas`). */
export interface AreaView {
  id: string;
  name: string | null;
  icon: string | null;
  order_key: string;
}

/** The projects + areas the Tasks view groups by (`tasks.projects_areas`). */
export interface ProjectsAreas {
  projects: ProjectView[];
  areas: AreaView[];
}

/** The status a task can be moved to (`tasks.set_status`). */
export type TaskStatus = "open" | "completed" | "canceled";

/**
 * A unified agenda event (`calendar.agenda`) — a read-only projection of a
 * persisted task/reminder/meeting into a calendar entry via the calendar crate's
 * projection fns. `source_id` is the originating entity id (jump-to-source).
 */
export interface AgendaEvent {
  uid: string;
  title: string;
  start_ms: number;
  end_ms: number;
  all_day: boolean;
  source: "task" | "reminder" | "meeting";
  source_id: string;
  status: "confirmed" | "tentative" | "cancelled";
  location: string | null;
  description: string | null;
}

/** One resolved citation on a grounded `AnswerV1` (Data Model §14.2). */
export interface Citation {
  chunk_id: string;
  source_kind: "note_block" | "transcript_window" | "task" | "reminder";
  source_id: string;
  t_start_ms: number | null;
  snippet: string;
}

/**
 * The evidence-cited answer (`ai.ask` → `AnswerV1`). When `unanswered` is true,
 * `citations` is empty and `answer` is the canonical refusal — display nothing
 * grounded. Every citation on a grounded answer is guaranteed to resolve (N14).
 */
export interface AnswerV1 {
  schema: string;
  answer: string;
  citations: Citation[];
  confidence: number;
  unanswered: boolean;
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

// ---- M2: meeting intelligence (HLD §8.4, Data Model §14.1) ----------------

/**
 * A transcript segment as pushed on `LiveTranscript` and returned by
 * `meeting.transcript` (app-domain `TranscriptSegment`). `pass` is `"live"` for
 * provisional pass-1 hypotheses (superseded) and `"final"` for the authoritative
 * pass-2 evidence anchors.
 */
export interface TranscriptSegmentT {
  segment_id: string;
  t_start_ms: number;
  t_end_ms: number;
  speaker: string | null;
  text: string;
  pass: "live" | "final";
  confidence: number | null;
}

/** A discussion topic with its supporting evidence (Data Model §14.1). */
export interface ArtifactTopic {
  title: string;
  summary: string;
  evidence_segment_ids: string[];
}

/** A decision reached, with optional rationale. */
export interface ArtifactDecision {
  statement: string;
  rationale: string | null;
  evidence_segment_ids: string[];
}

/** An action item — `owner`/`due_date` null unless stated in the cited evidence. */
export interface ArtifactActionItem {
  task: string;
  owner: string | null;
  due_date: string | null;
  evidence_segment_ids: string[];
}

/** A risk raised during the meeting. */
export interface ArtifactRisk {
  statement: string;
  evidence_segment_ids: string[];
}

/** An unresolved question left open. */
export interface ArtifactOpenQuestion {
  question: string;
  evidence_segment_ids: string[];
}

/** The immutable-per-generation meeting artifact (Data Model §14.1). */
export interface MeetingArtifactV1 {
  schema: string;
  session_id: string;
  executive_summary: string;
  topics: ArtifactTopic[];
  decisions: ArtifactDecision[];
  action_items: ArtifactActionItem[];
  risks: ArtifactRisk[];
  open_questions: ArtifactOpenQuestion[];
}

/** An application that can be selected as a capture source (`capture-api`). */
export interface CapturableAppT {
  app_id: string;
  display_name: string;
  executable: string | null;
  produces_audio: boolean;
}

/** Honest per-platform capture capabilities (HLD §9.1; capability honesty). */
export interface CaptureCapabilitiesT {
  platform: string;
  app_level_audio: "supported" | "best_effort" | "unsupported";
  exclude_self: boolean;
  microphone: boolean;
  system_fallback: "not_applicable" | "explicit_only" | "unavailable";
  health: { state: string; reason?: string };
}

/** The grant state of the OS permissions a capture needs. */
export interface PermissionReportT {
  screen_capture: string;
  microphone: string;
  portal: string;
  all_granted: boolean;
}

/** `meeting.preflight` — the capability + permission gate for the arm affordance. */
export interface PreflightReportT {
  capabilities: CaptureCapabilitiesT;
  permissions: PermissionReportT;
  ready: boolean;
}

/** A `session` row projection (`meeting.get`). */
export interface SessionViewT {
  id: string;
  state: string;
  note_id: string | null;
  started_at: number | null;
  ended_at: number | null;
  duration_ms: number | null;
  platform: string;
  degraded_reason: string | null;
}

/** One suggested `action_item` — the review surface before promotion to a Task. */
export interface ActionItemViewT {
  id: string;
  idx: number;
  task_text: string;
  owner_text: string | null;
  due_date: string | null;
  evidence_segment_ids: string[];
  status: string;
  promoted_task_id: string | null;
}

/** The `meeting.start` payload (echoes `app-service::MeetingConfig`). */
export interface MeetingStartConfig {
  sources: string[];
  captureMicrophone: boolean;
  title?: string | null;
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

  /** `tasks.set_status` → move a task to open/completed/canceled. */
  tasksSetStatus: (taskId: string, status: TaskStatus): Promise<TaskView> =>
    call<TaskView>("tasks_set_status", { task_id: taskId, status }),

  /** `tasks.projects_areas` → the projects + areas the Tasks view groups by. */
  tasksProjectsAreas: (): Promise<ProjectsAreas> =>
    call<ProjectsAreas>("tasks_projects_areas", {}),

  // ---- Calendar (read-mostly unified agenda projection) ------------------

  /** `calendar.agenda` → events in `[from_ms, to_ms]`, sorted by `start_ms`. */
  calendarAgenda: (fromMs: number, toMs: number): Promise<AgendaEvent[]> =>
    call<AgendaEvent[]>("calendar_agenda", { from_ms: fromMs, to_ms: toMs }),

  /** `calendar.export_ics` → an RFC 5545 ICS document for the window. */
  calendarExportIcs: (fromMs: number, toMs: number): Promise<string> =>
    call<string>("calendar_export_ics", { from_ms: fromMs, to_ms: toMs }),

  // ---- AI workspace (grounded, evidence-cited answers) -------------------

  /** `ai.ask` → an `AnswerV1`: a grounded answer + resolvable citations, or a
   * canonical refusal with `unanswered: true`. */
  aiAsk: (query: string): Promise<AnswerV1> => call<AnswerV1>("ai_ask", { query }),

  captureQuick: (text: string): Promise<CaptureResult> =>
    call<CaptureResult>("capture_quick", { text, kind_hint: null }),

  nlpParse: (text: string): Promise<ParsedEntry> => call<ParsedEntry>("nlp_parse", { text }),

  searchQuery: (q: string): Promise<SearchResults> =>
    call<SearchResults>("search_query", { q, mode: "go", limit: 20 }),

  // ---- M2: meeting intelligence (HLD §8.4) -------------------------------

  /** The applications available as capture sources for the picker. */
  meetingListApps: (): Promise<CapturableAppT[]> =>
    call<CapturableAppT[]>("meeting_list_apps", {}),

  /** `meeting.preflight` → honest capability + permission report (never records). */
  meetingPreflight: (sources: string[]): Promise<PreflightReportT> =>
    call<PreflightReportT>("meeting_preflight", { sources }),

  /** `meeting.start` → the new session id (NEW→…→RECORDING). */
  meetingStart: (config: MeetingStartConfig): Promise<string> =>
    call<string>("meeting_start", {
      sources: config.sources,
      capture_microphone: config.captureMicrophone,
      exclude_self: true,
      sample_rate_hz: 48_000,
      title: config.title ?? null,
    }),

  /** `meeting.pause` → the resulting `SessionState`. The LLM never owns this. */
  meetingPause: (sessionId: string): Promise<string> =>
    call<string>("meeting_pause", { session_id: sessionId }),

  /** `meeting.resume` → the resulting `SessionState`. */
  meetingResume: (sessionId: string): Promise<string> =>
    call<string>("meeting_resume", { session_id: sessionId }),

  /** `meeting.stop` → STOPPING→CAPTURED→…→COMPLETE (progress arrives via events). */
  meetingStop: (sessionId: string): Promise<string> =>
    call<string>("meeting_stop", { session_id: sessionId }),

  /** `meeting.get` → the current `session` row projection. */
  meetingGet: (sessionId: string): Promise<SessionViewT> =>
    call<SessionViewT>("meeting_get", { session_id: sessionId }),

  /** The persisted final transcript (pass-2 evidence anchors) for review. */
  meetingTranscript: (sessionId: string): Promise<TranscriptSegmentT[]> =>
    call<TranscriptSegmentT[]>("meeting_transcript", { session_id: sessionId }),

  /** `meeting.artifact` → the evidence-resolved `MeetingArtifactV1`. */
  meetingArtifact: (sessionId: string): Promise<MeetingArtifactV1> =>
    call<MeetingArtifactV1>("meeting_artifact", { session_id: sessionId }),

  /** The suggested action items for a session (the review surface). */
  meetingActionItems: (sessionId: string): Promise<ActionItemViewT[]> =>
    call<ActionItemViewT[]>("meeting_action_items", { session_id: sessionId }),

  /** `meeting.actionItemToTask` → the new Task id (writes `spawned_from` + evidence). */
  meetingActionItemToTask: (sessionId: string, actionItemId: string): Promise<string> =>
    call<string>("meeting_action_item_to_task", {
      session_id: sessionId,
      action_item_id: actionItemId,
      overrides: null,
    }),
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
