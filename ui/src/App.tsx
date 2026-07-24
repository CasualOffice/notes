/**
 * Casual Note — WebView shell root (main window).
 *
 * The WebView is a thin JSON/event view (Architecture §13.1 decision #1): it never
 * sees SQL, raw filesystem paths, or PCM. It talks to the Rust core only through
 * `invoke(cmd, args)` (HLD §6) and reconciles state from `AppEvent`s (HLD §7).
 *
 * M1 Notes experience: a notebooks/folder tree with a pinned Today entry on the
 * left, a Tiptap editor (custom nodes, slash menu, wikilink autocomplete, Markdown
 * I/O) in the centre, and a backlinks panel on the right — plus intent-routed quick
 * capture. Outside Tauri the IPC layer serves an in-memory sample store so the whole
 * app renders for preview.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { AskView } from "./features/ai";
import { Backlinks } from "./features/backlinks";
import { CalendarView } from "./features/calendar";
import { Editor } from "./features/editor";
import { Meetings } from "./features/meetings";
import { Sidebar } from "./features/notebooks";
import { QuickCapture } from "./features/quick-capture";
import { TasksView } from "./features/tasks";
import { api, isTauri, onAppEvent, type NotebookNode, type NoteSummary } from "./lib/api";

/** The top-level pillars surfaced in the shell nav, and their hash routes. */
const VIEWS = ["notes", "tasks", "calendar", "meetings", "ask"] as const;
type View = (typeof VIEWS)[number];

const VIEW_LABEL: Record<View, string> = {
  notes: "Notes",
  tasks: "Tasks",
  calendar: "Calendar",
  meetings: "Meetings",
  ask: "Ask",
};

/** The current view parsed from `location.hash` (defaults to Notes). */
function viewFromHash(): View {
  const h = typeof window === "undefined" ? "" : window.location.hash.replace(/^#\/?/, "");
  return (VIEWS as readonly string[]).includes(h) ? (h as View) : "notes";
}

const NOTE_EVENTS = new Set(["NoteSaved", "NoteProjected"]);
const NOTEBOOK_EVENTS = new Set(["NotebooksChanged"]);

function todayISO(): string {
  const d = new Date();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${m}-${day}`;
}

function headingDoc(title: string): string {
  return JSON.stringify({
    type: "doc",
    content: [{ type: "heading", attrs: { level: 1 }, content: [{ type: "text", text: title }] }],
  });
}

export function App(): React.JSX.Element {
  const [notebooks, setNotebooks] = useState<NotebookNode[]>([]);
  const [selectedNotebookId, setSelectedNotebookId] = useState<string | null>(null);
  const [notes, setNotes] = useState<NoteSummary[]>([]);
  const [allNotes, setAllNotes] = useState<NoteSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [view, setView] = useState<View>(() => viewFromHash());
  const [error, setError] = useState<string>("");
  const booted = useRef<boolean>(false);
  const notebookRef = useRef<string | null>(null);
  notebookRef.current = selectedNotebookId;

  const select = useCallback((id: string): void => {
    setSelectedId(id);
  }, []);

  // Deep-linking: keep `view` in sync with the URL hash so /#tasks, /#calendar,
  // /#ask, /#meetings deep-link and Back/Forward navigate between pillars.
  useEffect(() => {
    const onHash = (): void => setView(viewFromHash());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  const goto = useCallback((v: View): void => {
    setView(v);
    if (viewFromHash() !== v) window.location.hash = v;
  }, []);

  const openNote = useCallback(
    (id: string): void => {
      goto("notes");
      setSelectedId(id);
    },
    [goto],
  );

  const openSource = useCallback(
    (source: "task" | "reminder" | "meeting", _id: string): void => {
      goto(source === "meeting" ? "meetings" : "tasks");
    },
    [goto],
  );

  const refreshNotes = useCallback(async (): Promise<NoteSummary[]> => {
    const nb = notebookRef.current;
    const [filtered, all] = await Promise.all([
      api.notesList(nb),
      nb == null ? Promise.resolve<NoteSummary[] | null>(null) : api.notesList(null),
    ]);
    setNotes(filtered);
    setAllNotes(all ?? filtered);
    return filtered;
  }, []);

  const refreshNotebooks = useCallback(async (): Promise<void> => {
    setNotebooks(await api.notebooksList());
  }, []);

  // Boot: load notebooks + notes and open the most recent (or create the first).
  useEffect(() => {
    if (booted.current) return;
    booted.current = true;
    void (async () => {
      try {
        await refreshNotebooks();
        const list = await refreshNotes();
        const first = list[0]?.id ?? (await api.notesCreate());
        if (!list[0]) await refreshNotes();
        select(first);
      } catch (e: unknown) {
        setError(String(e));
      }
    })();
  }, [refreshNotebooks, refreshNotes, select]);

  // Reconcile from the single core→WebView event channel (HLD §7).
  useEffect(() => {
    const unlisten = onAppEvent((ev) => {
      if (NOTE_EVENTS.has(ev.type)) void refreshNotes().catch(() => undefined);
      if (NOTEBOOK_EVENTS.has(ev.type)) void refreshNotebooks().catch(() => undefined);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refreshNotes, refreshNotebooks]);

  const selectNotebook = useCallback(
    (id: string | null): void => {
      setSelectedNotebookId(id);
      notebookRef.current = id;
      void refreshNotes().catch((e: unknown) => setError(String(e)));
    },
    [refreshNotes],
  );

  const createNotebook = useCallback(
    (name: string, parentId: string | null): void => {
      void api
        .notebooksCreate(name, parentId)
        .then(() => refreshNotebooks())
        .catch((e: unknown) => setError(String(e)));
    },
    [refreshNotebooks],
  );

  const createNote = useCallback(async (): Promise<void> => {
    try {
      const id = await api.notesCreate(undefined, selectedNotebookId);
      await refreshNotes();
      select(id);
    } catch (e: unknown) {
      setError(String(e));
    }
  }, [refreshNotes, select, selectedNotebookId]);

  const openToday = useCallback((): void => {
    void api
      .dailyGetOrCreate(todayISO())
      .then(async (note) => {
        await refreshNotes();
        select(note.id);
      })
      .catch((e: unknown) => setError(String(e)));
  }, [refreshNotes, select]);

  const openWikilink = useCallback(
    (target: string, targetId: string | null): void => {
      if (targetId) {
        select(targetId);
        return;
      }
      const existing = allNotes.find((n) => (n.title ?? "").toLowerCase() === target.toLowerCase());
      if (existing) {
        select(existing.id);
        return;
      }
      void api
        .notesCreate(headingDoc(target), selectedNotebookId)
        .then(async (id) => {
          await refreshNotes();
          select(id);
        })
        .catch((e: unknown) => setError(String(e)));
    },
    [allNotes, refreshNotes, select, selectedNotebookId],
  );

  const selectedSummary = allNotes.find((n) => n.id === selectedId) ?? notes.find((n) => n.id === selectedId);
  const todayActive = selectedSummary?.daily_date === todayISO();

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark">Casual Note</span>
          <span className="brand-sub">{VIEW_LABEL[view]}</span>
        </div>
        <nav className="nav-tabs" aria-label="Pillars">
          {VIEWS.map((v) => (
            <button
              key={v}
              type="button"
              className={`nav-tab${view === v ? " active" : ""}`}
              aria-current={view === v}
              onClick={() => goto(v)}
            >
              {VIEW_LABEL[v]}
            </button>
          ))}
        </nav>
        {view === "notes" && (
          <QuickCapture
            onCaptured={(r) => {
              void refreshNotes().catch(() => undefined);
              if (r.entity_ref.kind === "note") select(r.entity_ref.id);
            }}
          />
        )}
        <span className="mode-pill">{isTauri ? "Local store" : "Preview (sample data)"}</span>
      </header>

      {error && (
        <div className="error-banner" role="alert">
          {error}
        </div>
      )}

      {view === "meetings" ? (
        <Meetings />
      ) : view === "tasks" ? (
        <TasksView />
      ) : view === "calendar" ? (
        <CalendarView onOpenSource={openSource} />
      ) : view === "ask" ? (
        <AskView onOpenNote={openNote} />
      ) : (
        <div className="workspace">
        <Sidebar
          notebooks={notebooks}
          selectedNotebookId={selectedNotebookId}
          onSelectNotebook={selectNotebook}
          onCreateNotebook={createNotebook}
          notes={notes}
          selectedNoteId={selectedId}
          onSelectNote={select}
          onCreateNote={() => void createNote()}
          onOpenToday={openToday}
          todayActive={todayActive}
        />

        {selectedId ? (
          <div className="main-pane">
            <Editor
              key={selectedId}
              noteId={selectedId}
              notes={allNotes}
              notebooks={notebooks}
              onOpenNote={select}
              onOpenWikilink={openWikilink}
              onChanged={() => void refreshNotes().catch(() => undefined)}
            />
            <Backlinks noteId={selectedId} onOpen={select} />
          </div>
        ) : (
          <div className="editor-pane">
            <div className="editor-empty">
              <p>Select a note, or create a new one to start writing.</p>
              <button type="button" className="btn btn-accent" onClick={() => void createNote()}>
                New note
              </button>
            </div>
          </div>
        )}
        </div>
      )}
    </div>
  );
}
