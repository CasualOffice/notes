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
  ActionItemViewT,
  AgendaEvent,
  AnswerV1,
  AppEventEnvelope,
  AreaView,
  BacklinkRef,
  CapturableAppT,
  CaptureResult,
  Citation,
  MeetingArtifactV1,
  Note,
  NotebookNode,
  NoteSummary,
  NoteView,
  ParsedEntry,
  PreflightReportT,
  ProjectsAreas,
  ProjectView,
  SaveResult,
  SearchResults,
  SessionViewT,
  TaskStatus,
  TaskView,
  TranscriptSegmentT,
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

/** Optional fields when seeding/creating a mock task. */
interface TaskSeed {
  priority?: number;
  someday?: boolean;
  startOn?: string;
  deadlineOn?: string;
  projectId?: string;
  areaId?: string;
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

/** Local `YYYY-MM-DD` for `today + days` (parity with the core's local-date keys). */
function isoDay(days = 0): string {
  const d = new Date();
  d.setDate(d.getDate() + days);
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${m}-${day}`;
}

/** Epoch-ms at the local wall-clock `hh:mm` on `today + days`. */
function atClock(days: number, hh: number, mm = 0): number {
  const d = new Date();
  d.setDate(d.getDate() + days);
  d.setHours(hh, mm, 0, 0);
  return d.getTime();
}

/** Epoch-ms at local midnight for a `YYYY-MM-DD` day (all-day event anchor). */
function dayToMs(iso: string): number {
  return new Date(`${iso}T00:00:00`).getTime();
}

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

// ---- Meeting intelligence fixtures (M2 preview) ---------------------------

/** The mock capturable applications the source picker lists. */
const CAPTURABLE_APPS: CapturableAppT[] = [
  { app_id: "us.zoom.xos", display_name: "Zoom", executable: null, produces_audio: true },
  { app_id: "com.tinyspeck.slackmacgap", display_name: "Slack huddle", executable: null, produces_audio: true },
  { app_id: "com.google.Chrome", display_name: "Google Meet (Chrome)", executable: null, produces_audio: true },
  { app_id: "com.microsoft.teams2", display_name: "Microsoft Teams", executable: null, produces_audio: false },
];

/** One line of the canned meeting script (speaker + spoken text). */
interface ScriptLine {
  speaker: string;
  text: string;
}

const MEETING_SCRIPT: ScriptLine[] = [
  { speaker: "Alex", text: "Let's start with the Q3 roadmap. The biggest theme is reducing capture friction." },
  { speaker: "Priya", text: "Agreed. Users drop off when quick capture takes more than about two seconds." },
  { speaker: "Alex", text: "So the decision is we ship the sub-two-second capture target for the beta." },
  { speaker: "Sam", text: "I can own the capture latency work. I'll have a prototype by next Friday." },
  { speaker: "Priya", text: "We also need to improve search recall — fusing vector results with FTS." },
  { speaker: "Alex", text: "Let's make Priya the owner of the search fusion evaluation, due end of month." },
  { speaker: "Sam", text: "One risk: local inference might not hit our latency target on older machines." },
  { speaker: "Priya", text: "Open question — how far do we push local models before the first model download?" },
];

const SEGMENT_MS = 6000;

/** A fully-formed meeting: final segments + the artifact + suggested action items,
 * with every evidence id resolving to a real segment (parity with the coordinator's
 * "evidence or nothing" cleaning). Fresh ids per session so nothing collides. */
interface MeetingFixture {
  segments: TranscriptSegmentT[];
  artifact: MeetingArtifactV1;
  actionItems: ActionItemViewT[];
}

function makeMeetingFixture(sessionId: string): MeetingFixture {
  const segments: TranscriptSegmentT[] = MEETING_SCRIPT.map((line, i) => ({
    segment_id: uuid(),
    t_start_ms: i * SEGMENT_MS,
    t_end_ms: (i + 1) * SEGMENT_MS,
    speaker: line.speaker,
    text: line.text,
    pass: "final" as const,
    confidence: 0.9,
  }));
  const seg = (i: number): string => segments[i]?.segment_id ?? "";

  const artifact: MeetingArtifactV1 = {
    schema: "MeetingArtifactV1",
    session_id: sessionId,
    executive_summary:
      "The team aligned the Q3 roadmap around reducing capture friction and improving " +
      "search recall — committing to a sub-two-second quick-capture target for the beta " +
      "and a hybrid FTS-plus-vector search evaluation.",
    topics: [
      {
        title: "Reducing capture friction",
        summary: "Capture friction is the headline Q3 theme; users abandon capture past two seconds.",
        evidence_segment_ids: [seg(0), seg(1), seg(2)],
      },
      {
        title: "Improving search recall",
        summary: "Search recall improves by fusing vector similarity with the existing FTS index.",
        evidence_segment_ids: [seg(4), seg(5)],
      },
    ],
    decisions: [
      {
        statement: "Ship a sub-two-second quick-capture target for the beta.",
        rationale: "Users drop off when capture takes longer than about two seconds.",
        evidence_segment_ids: [seg(1), seg(2)],
      },
    ],
    action_items: [
      {
        task: "Prototype the capture-latency work",
        owner: "Sam",
        due_date: null,
        evidence_segment_ids: [seg(3)],
      },
      {
        task: "Run the FTS + vector search-fusion evaluation",
        owner: "Priya",
        due_date: "2026-07-31",
        evidence_segment_ids: [seg(5)],
      },
    ],
    risks: [
      {
        statement: "Local inference may not meet the latency target on older machines.",
        evidence_segment_ids: [seg(6)],
      },
    ],
    open_questions: [
      {
        question: "How far should local models be pushed before the first model download?",
        evidence_segment_ids: [seg(7)],
      },
    ],
  };

  const actionItems: ActionItemViewT[] = artifact.action_items.map((ai, idx) => ({
    id: uuid(),
    idx,
    task_text: ai.task,
    owner_text: ai.owner,
    due_date: ai.due_date,
    evidence_segment_ids: ai.evidence_segment_ids,
    status: "suggested",
    promoted_task_id: null,
  }));

  return { segments, artifact, actionItems };
}

/** The live state of one simulated meeting session. */
interface MeetingRuntime {
  id: string;
  state: string;
  title: string | null;
  fixture: MeetingFixture;
  liveIdx: number;
  startedAt: number;
  levelTimer: ReturnType<typeof setInterval> | null;
  liveTimer: ReturnType<typeof setInterval> | null;
}

class MockCore {
  private notes = new Map<string, NoteView>();
  private notebooks: NotebookRow[] = [];
  private tasks: TaskView[] = [];
  private areas: AreaView[] = [];
  private projects: ProjectView[] = [];
  private extraEvents: AgendaEvent[] = [];
  private reminders = new Set<string>();
  private meetings = new Map<string, MeetingRuntime>();
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

    // Areas + projects the Tasks view groups by.
    const workArea: AreaView = { id: uuid(), name: "Work", icon: null, order_key: "a0" };
    const homeArea: AreaView = { id: uuid(), name: "Home", icon: null, order_key: "a1" };
    this.areas = [workArea, homeArea];
    const launch: ProjectView = {
      id: uuid(),
      name: "Q3 Launch",
      area_id: workArea.id,
      status: "open",
      order_key: "a0",
    };
    const homeOps: ProjectView = {
      id: uuid(),
      name: "Home ops",
      area_id: homeArea.id,
      status: "open",
      order_key: "a0",
    };
    this.projects = [launch, homeOps];

    // Tasks spanning every derived bucket (Feature Specs §3.1). Buckets are queries
    // over these fields — no stored bucket state.
    this.tasks = [
      // Today: a deadline that has arrived, and a task scheduled for the past.
      this.makeTask("Review the M2 acceptance checklist", {
        priority: 2,
        deadlineOn: isoDay(0),
        projectId: launch.id,
        areaId: workArea.id,
      }),
      this.makeTask("Reply to Priya about search fusion", {
        priority: 1,
        startOn: isoDay(-1),
        projectId: launch.id,
        areaId: workArea.id,
      }),
      // Upcoming: scheduled ahead, or a future deadline with no start.
      this.makeTask("Prepare the beta release notes", {
        startOn: isoDay(3),
        projectId: launch.id,
        areaId: workArea.id,
      }),
      this.makeTask("Renew the casualnote.app domain", {
        deadlineOn: isoDay(6),
        areaId: homeArea.id,
      }),
      // Anytime: actionable, undated.
      this.makeTask("Wire the tray menu actions", { projectId: launch.id, areaId: workArea.id }),
      this.makeTask("Sketch the onboarding illustration", { areaId: workArea.id }),
      // Someday: parked until activated.
      this.makeTask("Explore the Parakeet speech backend", {
        someday: true,
        areaId: workArea.id,
      }),
      this.makeTask("Read 'Thinking in Systems'", { someday: true, areaId: homeArea.id }),
    ];

    // Reminders + meetings projected into the unified agenda. Persisted tasks are
    // projected dynamically (so newly-created dated tasks appear); these fixed
    // sources are seeded once, anchored to the current week.
    this.extraEvents = [
      {
        uid: `rem-${uuid()}`,
        title: "Daily standup",
        start_ms: atClock(0, 9, 15),
        end_ms: atClock(0, 9, 30),
        all_day: false,
        source: "reminder",
        source_id: uuid(),
        status: "confirmed",
        location: null,
        description: "Recurring team standup.",
      },
      {
        uid: `mtg-${uuid()}`,
        title: "Q3 roadmap sync",
        start_ms: atClock(0, 10),
        end_ms: atClock(0, 11),
        all_day: false,
        source: "meeting",
        source_id: uuid(),
        status: "confirmed",
        location: "Zoom",
        description: "Align on the Q3 roadmap themes.",
      },
      {
        uid: `rem-${uuid()}`,
        title: "Call Sam about capture latency",
        start_ms: atClock(1, 15),
        end_ms: atClock(1, 15, 15),
        all_day: false,
        source: "reminder",
        source_id: uuid(),
        status: "confirmed",
        location: null,
        description: null,
      },
      {
        uid: `mtg-${uuid()}`,
        title: "Design review",
        start_ms: atClock(2, 14),
        end_ms: atClock(2, 15),
        all_day: false,
        source: "meeting",
        source_id: uuid(),
        status: "tentative",
        location: "Google Meet",
        description: "Walk through the calendar agenda designs.",
      },
      {
        uid: `mtg-${uuid()}`,
        title: "1:1 with Priya",
        start_ms: atClock(4, 13),
        end_ms: atClock(4, 13, 30),
        all_day: false,
        source: "meeting",
        source_id: uuid(),
        status: "confirmed",
        location: null,
        description: null,
      },
    ];
  }

  private makeTask(title: string, opts: TaskSeed = {}): TaskView {
    return {
      id: uuid(),
      title,
      project_id: opts.projectId ?? null,
      area_id: opts.areaId ?? null,
      notes_md: null,
      status: "open",
      priority: opts.priority ?? 0,
      someday: opts.someday ?? false,
      start_on: opts.startOn ?? null,
      deadline_on: opts.deadlineOn ?? null,
      completed_at: null,
      order_key: String(this.tasks.length + 1).padStart(4, "0"),
    };
  }

  /** Derived-bucket membership (Feature Specs §3.1) — a query over task fields. */
  private bucketTasks(bucket: string): TaskView[] {
    const today = isoDay(0);
    const open = this.tasks.filter((t) => t.status === "open");
    const inToday = (t: TaskView): boolean =>
      !t.someday &&
      ((t.start_on != null && t.start_on <= today) ||
        (t.deadline_on != null && t.deadline_on <= today));
    switch (bucket) {
      case "Today":
        return open.filter(inToday);
      case "Upcoming":
        return open.filter(
          (t) =>
            !t.someday &&
            !inToday(t) &&
            ((t.start_on != null && t.start_on > today) ||
              (t.start_on == null && t.deadline_on != null && t.deadline_on > today)),
        );
      case "Anytime":
        return open.filter((t) => !t.someday && t.start_on == null && t.deadline_on == null);
      case "Someday":
        return open.filter((t) => t.someday);
      default:
        return [];
    }
  }

  private setTaskStatus(taskId: string, status: TaskStatus): TaskView {
    const t = this.tasks.find((x) => x.id === taskId);
    if (!t) throw new Error("task not found");
    t.status = status;
    t.completed_at = status === "completed" ? this.now() : null;
    this.emit("TaskChanged", { task_id: t.id });
    return t;
  }

  /** Persisted dated tasks projected into all-day agenda events (jump-to-source). */
  private taskEvents(): AgendaEvent[] {
    const out: AgendaEvent[] = [];
    for (const t of this.tasks) {
      if (t.status !== "open" || t.someday) continue;
      const day = t.deadline_on ?? t.start_on;
      if (!day) continue;
      const start = dayToMs(day);
      out.push({
        uid: `task-${t.id}`,
        title: t.title ?? "Untitled task",
        start_ms: start,
        end_ms: start + 24 * 3_600_000,
        all_day: true,
        source: "task",
        source_id: t.id,
        status: "confirmed",
        location: null,
        description: t.deadline_on ? "Deadline" : "Scheduled",
      });
    }
    return out;
  }

  /** The merged, window-clipped, start-sorted agenda (`calendar.agenda`). */
  private agenda(fromMs: number, toMs: number): AgendaEvent[] {
    return [...this.taskEvents(), ...this.extraEvents]
      .filter((e) => e.start_ms <= toMs && e.end_ms >= fromMs)
      .sort((a, b) => a.start_ms - b.start_ms);
  }

  /** A minimal RFC 5545 ICS document for the window (`calendar.export_ics`). */
  private exportIcs(fromMs: number, toMs: number): string {
    const two = (n: number): string => String(n).padStart(2, "0");
    const icsDay = (ms: number): string => {
      const d = new Date(ms);
      return `${d.getFullYear()}${two(d.getMonth() + 1)}${two(d.getDate())}`;
    };
    const icsUtc = (ms: number): string => {
      const d = new Date(ms);
      return (
        `${d.getUTCFullYear()}${two(d.getUTCMonth() + 1)}${two(d.getUTCDate())}` +
        `T${two(d.getUTCHours())}${two(d.getUTCMinutes())}${two(d.getUTCSeconds())}Z`
      );
    };
    const lines = ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Casual Note//Agenda//EN", "CALSCALE:GREGORIAN"];
    for (const e of this.agenda(fromMs, toMs)) {
      lines.push("BEGIN:VEVENT", `UID:${e.uid}`, `SUMMARY:${e.title}`);
      if (e.all_day) {
        lines.push(`DTSTART;VALUE=DATE:${icsDay(e.start_ms)}`, `DTEND;VALUE=DATE:${icsDay(e.end_ms)}`);
      } else {
        lines.push(`DTSTART:${icsUtc(e.start_ms)}`, `DTEND:${icsUtc(e.end_ms)}`);
      }
      lines.push(`STATUS:${e.status.toUpperCase()}`);
      if (e.location) lines.push(`LOCATION:${e.location}`);
      if (e.description) lines.push(`DESCRIPTION:${e.description}`);
      lines.push("END:VEVENT");
    }
    lines.push("END:VCALENDAR");
    return lines.join("\r\n");
  }

  /** A grounded, evidence-cited `AnswerV1`, or an honest refusal (`ai.ask`). */
  private ask(query: string): AnswerV1 {
    const q = query.trim().toLowerCase();
    const GROUND = [
      "capture",
      "search",
      "roadmap",
      "meeting",
      "beta",
      "latency",
      "task",
      "q3",
      "recall",
      "friction",
      "priya",
      "sam",
    ];
    const grounded = q.length > 0 && GROUND.some((k) => q.includes(k));
    if (!grounded) {
      return {
        schema: "AnswerV1",
        answer: "I don't have enough grounded evidence in your notes to answer that.",
        citations: [],
        confidence: 0,
        unanswered: true,
      };
    }
    const notes = [...this.notes.values()];
    const roadmap = notes.find((n) => (n.title ?? "").toLowerCase().includes("roadmap")) ?? notes[0];
    const citations: Citation[] = [];
    if (roadmap) {
      citations.push({
        chunk_id: uuid(),
        source_kind: "note_block",
        source_id: roadmap.id,
        t_start_ms: null,
        snippet: firstBodyText(roadmap.doc_json) || (roadmap.title ?? ""),
      });
    }
    citations.push({
      chunk_id: uuid(),
      source_kind: "transcript_window",
      source_id: uuid(),
      t_start_ms: 12_000,
      snippet: "So the decision is we ship the sub-two-second capture target for the beta.",
    });
    return {
      schema: "AnswerV1",
      answer:
        "The Q3 focus is reducing capture friction — the team committed to a sub-two-second " +
        "quick-capture target for the beta — and improving search recall by fusing vector " +
        "similarity with the existing FTS index.",
      citations,
      confidence: 0.78,
      unanswered: false,
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
        return this.bucketTasks(String(args["bucket"]));
      case "tasks_create": {
        const input = args["input"] as {
          title: string;
          project_id?: string | null;
          area_id?: string | null;
          start_on?: string | null;
          deadline_on?: string | null;
          someday?: boolean;
          priority?: number;
        };
        const seed: TaskSeed = {};
        if (input.project_id != null) seed.projectId = input.project_id;
        if (input.area_id != null) seed.areaId = input.area_id;
        if (input.start_on != null) seed.startOn = input.start_on;
        if (input.deadline_on != null) seed.deadlineOn = input.deadline_on;
        if (input.someday != null) seed.someday = input.someday;
        if (input.priority != null) seed.priority = input.priority;
        const t = this.makeTask(input.title, seed);
        this.tasks.push(t);
        this.emit("TaskChanged", { task_id: t.id });
        return t;
      }
      case "tasks_complete":
        return this.setTaskStatus(String(args["task_id"]), "completed");
      case "tasks_set_status":
        return this.setTaskStatus(String(args["task_id"]), String(args["status"]) as TaskStatus);
      case "tasks_projects_areas":
        return { projects: this.projects, areas: this.areas } satisfies ProjectsAreas;
      case "calendar_agenda":
        return this.agenda(Number(args["from_ms"]), Number(args["to_ms"]));
      case "calendar_export_ics":
        return this.exportIcs(Number(args["from_ms"]), Number(args["to_ms"]));
      case "ai_ask":
        return this.ask(String(args["query"]));
      case "capture_quick": {
        const text_ = String(args["text"]);
        const parsed = parse(text_);
        const ref = this.route(parsed, text_);
        return { entity_ref: ref, parsed } satisfies CaptureResult;
      }
      case "nlp_parse":
        return parse(String(args["text"]));

      // ---- M2: meeting intelligence ------------------------------------
      case "meeting_list_apps":
        return CAPTURABLE_APPS;
      case "meeting_preflight":
        return this.meetingPreflight();
      case "meeting_start":
        return this.meetingStart(args);
      case "meeting_pause":
        return this.meetingPause(String(args["session_id"]));
      case "meeting_resume":
        return this.meetingResume(String(args["session_id"]));
      case "meeting_stop":
        return this.meetingStop(String(args["session_id"]));
      case "meeting_get":
        return this.meetingView(this.meeting(String(args["session_id"])));
      case "meeting_transcript":
        return this.meeting(String(args["session_id"])).fixture.segments;
      case "meeting_artifact":
        return this.meeting(String(args["session_id"])).fixture.artifact;
      case "meeting_action_items":
        return this.meeting(String(args["session_id"])).fixture.actionItems;
      case "meeting_action_item_to_task":
        return this.meetingActionItemToTask(
          String(args["session_id"]),
          String(args["action_item_id"]),
        );
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

  // ---- Meeting simulation -------------------------------------------------

  private meeting(id: string): MeetingRuntime {
    const rt = this.meetings.get(id);
    if (!rt) throw new Error(`session ${id} not found`);
    return rt;
  }

  private meetingView(rt: MeetingRuntime): SessionViewT {
    const ended = rt.state === "COMPLETE" ? rt.startedAt + MEETING_SCRIPT.length * SEGMENT_MS : null;
    return {
      id: rt.id,
      state: rt.state,
      note_id: rt.state === "COMPLETE" ? uuid() : null,
      started_at: rt.startedAt,
      ended_at: ended,
      duration_ms: ended ? ended - rt.startedAt : null,
      platform: "linux",
      degraded_reason: null,
    };
  }

  /** Honest preflight — everything granted so the picker can arm in preview. */
  private meetingPreflight(): PreflightReportT {
    return {
      capabilities: {
        platform: "linux",
        app_level_audio: "best_effort",
        exclude_self: true,
        microphone: true,
        system_fallback: "explicit_only",
        health: { state: "ready" },
      },
      permissions: {
        screen_capture: "granted",
        microphone: "granted",
        portal: "granted",
        all_granted: true,
      },
      ready: true,
    };
  }

  /** Move a session to a new state and push `SessionStateChanged` (HLD §7). */
  private meetingTransition(rt: MeetingRuntime, to: string): void {
    const from = rt.state;
    rt.state = to;
    this.emit("SessionStateChanged", { session_id: rt.id, from, to, degraded: null });
  }

  private clearMeetingTimers(rt: MeetingRuntime): void {
    if (rt.levelTimer !== null) clearInterval(rt.levelTimer);
    if (rt.liveTimer !== null) clearInterval(rt.liveTimer);
    rt.levelTimer = null;
    rt.liveTimer = null;
  }

  /** Emit a throttled level meter + stream the next provisional (pass-1) line. */
  private startMeetingTimers(rt: MeetingRuntime): void {
    this.clearMeetingTimers(rt);
    rt.levelTimer = setInterval(() => {
      // A lively but bounded RMS meter, in dBFS (−48..−8).
      const rms = -34 + Math.sin(Date.now() / 220) * 12 + (Math.random() * 8 - 4);
      this.emit("CaptureLevel", { session_id: rt.id, rms_dbfs: Math.max(-60, Math.min(-6, rms)) });
    }, 250);
    rt.liveTimer = setInterval(() => {
      const line = MEETING_SCRIPT[rt.liveIdx];
      if (!line) return; // script exhausted — meter keeps running until stop
      this.emit("LiveTranscript", {
        session_id: rt.id,
        segment: {
          segment_id: uuid(),
          t_start_ms: rt.liveIdx * SEGMENT_MS,
          t_end_ms: (rt.liveIdx + 1) * SEGMENT_MS,
          speaker: line.speaker,
          text: line.text,
          pass: "live",
          confidence: 0.62,
        } satisfies TranscriptSegmentT,
      });
      rt.liveIdx += 1;
    }, 1100);
  }

  private meetingStart(args: Record<string, unknown>): string {
    const id = uuid();
    const rt: MeetingRuntime = {
      id,
      state: "NEW",
      title: (args["title"] as string | null) ?? null,
      fixture: makeMeetingFixture(id),
      liveIdx: 0,
      startedAt: this.now(),
      levelTimer: null,
      liveTimer: null,
    };
    this.meetings.set(id, rt);
    // NEW → PREFLIGHT → READY → RECORDING (the LLM never owns this path). Deferred
    // one macrotask so these state events land *after* the caller has the session id
    // and has begun listening — otherwise the WebView would filter its own start.
    setTimeout(() => {
      this.meetingTransition(rt, "PREFLIGHT");
      this.meetingTransition(rt, "READY");
      this.meetingTransition(rt, "RECORDING");
      this.startMeetingTimers(rt);
    }, 0);
    return id;
  }

  private meetingPause(id: string): string {
    const rt = this.meeting(id);
    if (rt.state === "RECORDING") {
      this.clearMeetingTimers(rt);
      this.meetingTransition(rt, "PAUSED");
    }
    return rt.state;
  }

  private meetingResume(id: string): string {
    const rt = this.meeting(id);
    if (rt.state === "PAUSED") {
      this.meetingTransition(rt, "RECORDING");
      this.startMeetingTimers(rt);
    }
    return rt.state;
  }

  /**
   * Stop → drive the tail of the pipeline (STOPPING → CAPTURED →
   * FINAL_TRANSCRIBING → GENERATING → INDEXING → COMPLETE) over time, streaming the
   * authoritative final segments, then `ArtifactReady` + `IndexingProgress`. Capture
   * has already completed, so a slow generation never rewinds recording.
   */
  private meetingStop(id: string): string {
    const rt = this.meeting(id);
    if (rt.state !== "RECORDING" && rt.state !== "PAUSED") return rt.state;
    this.clearMeetingTimers(rt);
    this.meetingTransition(rt, "STOPPING");

    const step = (ms: number, fn: () => void): void => {
      setTimeout(() => {
        try {
          fn();
        } catch {
          /* preview-only: ignore */
        }
      }, ms);
    };

    step(200, () => this.meetingTransition(rt, "CAPTURED"));
    step(350, () => this.meetingTransition(rt, "FINAL_TRANSCRIBING"));
    // Stream the final (pass-2) segments as the authoritative evidence lands.
    rt.fixture.segments.forEach((s, i) => {
      step(500 + i * 180, () => this.emit("LiveTranscript", { session_id: rt.id, segment: s }));
    });
    const afterFinal = 500 + rt.fixture.segments.length * 180 + 250;
    step(afterFinal, () => this.meetingTransition(rt, "GENERATING"));
    step(afterFinal + 500, () => {
      this.emit("ArtifactReady", { session_id: rt.id });
      this.meetingTransition(rt, "INDEXING");
    });
    step(afterFinal + 650, () =>
      this.emit("IndexingProgress", { session_id: rt.id, stage: "note", pct: 0.3 }),
    );
    step(afterFinal + 800, () =>
      this.emit("IndexingProgress", { session_id: rt.id, stage: "action_items", pct: 0.7 }),
    );
    step(afterFinal + 950, () => {
      this.emit("IndexingProgress", { session_id: rt.id, stage: "complete", pct: 1.0 });
      this.meetingTransition(rt, "COMPLETE");
    });
    return rt.state;
  }

  private meetingActionItemToTask(sessionId: string, actionItemId: string): string {
    const rt = this.meeting(sessionId);
    const ai = rt.fixture.actionItems.find((x) => x.id === actionItemId);
    if (!ai) throw new Error(`action_item ${actionItemId} not found`);
    if (ai.promoted_task_id) return ai.promoted_task_id; // idempotent
    const t = this.makeTask(ai.task_text);
    if (ai.due_date) t.deadline_on = ai.due_date;
    this.tasks.push(t);
    ai.status = "promoted";
    ai.promoted_task_id = t.id;
    this.emit("TaskChanged", { task_id: t.id });
    return t.id;
  }

  private route(parsed: ParsedEntry, raw: string): { kind: string; id: string } {
    if (parsed.kind === "task") {
      const t = this.makeTask(parsed.title, { priority: parsed.priority });
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
