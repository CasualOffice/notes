/**
 * Casual Note — WebView shell root.
 *
 * The WebView is a thin JSON/event view (Architecture §13.1 decision #1): it never
 * sees SQL, raw filesystem paths, or PCM. It talks to the Rust core only through
 * `invoke(cmd, args)` (HLD §6) and reconciles state from `AppEvent`s (HLD §7).
 *
 * Phase-1 proves the round-trip end to end: a Tiptap editor bound to `notes.save`,
 * a task list driven by `tasks.bucket`/`tasks.complete`, and a quick-capture input
 * calling `capture.quick` (NLP-routed).
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { Editor } from "./features/editor/Editor";
import { api, onAppEvent, type AppEventEnvelope, type TaskView } from "./lib/api";

export function App(): React.JSX.Element {
  const [noteId, setNoteId] = useState<string | null>(null);
  const [tasks, setTasks] = useState<TaskView[]>([]);
  const [capture, setCapture] = useState<string>("");
  const [newTask, setNewTask] = useState<string>("");
  const [events, setEvents] = useState<AppEventEnvelope[]>([]);
  const [error, setError] = useState<string>("");
  const booted = useRef<boolean>(false);

  const refreshTasks = useCallback(async (): Promise<void> => {
    try {
      setTasks(await api.tasksBucket("Anytime"));
    } catch (e: unknown) {
      setError(String(e));
    }
  }, []);

  // Boot: pick an existing note or create one, then load tasks.
  useEffect(() => {
    if (booted.current) return;
    booted.current = true;
    void (async () => {
      try {
        const list = await api.notesList();
        const id = list[0]?.id ?? (await api.notesCreate());
        setNoteId(id);
        await refreshTasks();
      } catch (e: unknown) {
        setError(String(e));
      }
    })();
  }, [refreshTasks]);

  // Subscribe to the single core→WebView event channel (HLD §7).
  useEffect(() => {
    const unlisten = onAppEvent((ev) => {
      setEvents((prev) => [ev, ...prev].slice(0, 12));
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  const doCapture = async (): Promise<void> => {
    const text = capture.trim();
    if (!text) return;
    try {
      await api.captureQuick(text);
      setCapture("");
      await refreshTasks();
    } catch (e: unknown) {
      setError(String(e));
    }
  };

  const addTask = async (): Promise<void> => {
    const title = newTask.trim();
    if (!title) return;
    try {
      await api.tasksCreate(title);
      setNewTask("");
      await refreshTasks();
    } catch (e: unknown) {
      setError(String(e));
    }
  };

  const complete = async (id: string): Promise<void> => {
    try {
      await api.tasksComplete(id);
      await refreshTasks();
    } catch (e: unknown) {
      setError(String(e));
    }
  };

  return (
    <main style={{ maxWidth: 960, margin: "0 auto", padding: 24, fontFamily: "system-ui" }}>
      <h1 style={{ fontSize: 22 }}>Casual Note</h1>
      {error && (
        <p style={{ color: "crimson", fontSize: 13 }} role="alert">
          {error}
        </p>
      )}

      <section style={{ margin: "12px 0" }}>
        <label htmlFor="capture" style={{ fontSize: 13, fontWeight: 600 }}>
          Quick capture (NLP-routed → task / note / reminder)
        </label>
        <div style={{ display: "flex", gap: 8, marginTop: 4 }}>
          <input
            id="capture"
            value={capture}
            placeholder='e.g. "call Sam tomorrow 3pm #work !2"'
            onChange={(e) => setCapture(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void doCapture();
            }}
            style={{ flex: 1, padding: 6 }}
          />
          <button type="button" onClick={() => void doCapture()}>
            Capture
          </button>
        </div>
      </section>

      {noteId ? <Editor noteId={noteId} /> : <p>Preparing note…</p>}

      <section style={{ marginTop: 16, border: "1px solid #ccc", borderRadius: 8, padding: 12 }}>
        <h2 style={{ margin: 0, fontSize: 16 }}>Tasks · Anytime</h2>
        <div style={{ display: "flex", gap: 8, margin: "8px 0" }}>
          <input
            value={newTask}
            placeholder="New task title"
            onChange={(e) => setNewTask(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void addTask();
            }}
            style={{ flex: 1, padding: 6 }}
          />
          <button type="button" onClick={() => void addTask()}>
            Add
          </button>
        </div>
        <ul style={{ listStyle: "none", padding: 0, margin: 0 }}>
          {tasks.map((t) => (
            <li
              key={t.id}
              style={{ display: "flex", justifyContent: "space-between", padding: "4px 0" }}
            >
              <span>{t.title ?? "(untitled)"}</span>
              <button type="button" onClick={() => void complete(t.id)}>
                Complete
              </button>
            </li>
          ))}
          {tasks.length === 0 && <li style={{ opacity: 0.6 }}>No tasks yet.</li>}
        </ul>
      </section>

      <section style={{ marginTop: 16 }}>
        <h2 style={{ fontSize: 13, opacity: 0.7 }}>AppEvents (live)</h2>
        <ul style={{ fontSize: 12, fontFamily: "monospace", opacity: 0.75 }}>
          {events.map((ev) => (
            <li key={ev.seq}>
              #{ev.seq} {ev.type}
            </li>
          ))}
        </ul>
      </section>
    </main>
  );
}
