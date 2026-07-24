/**
 * The Tasks pillar (Feature Specs §3) — Things-style derived views. The four
 * buckets Today / Upcoming / Anytime / Someday are *queries over task fields*, not
 * stored states: each is read via `tasks.list(bucket)`. Quick-add creates a task;
 * the checkbox moves it through `tasks.set_status`. The WebView owns no task state —
 * it re-reads the buckets whenever the core emits `TaskChanged`.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { api, onAppEvent, type Bucket, type TaskView } from "../../lib/api";

const BUCKETS: { id: Bucket; label: string; blurb: string }[] = [
  { id: "Today", label: "Today", blurb: "Scheduled for today, or due." },
  { id: "Upcoming", label: "Upcoming", blurb: "Starts or is due later." },
  { id: "Anytime", label: "Anytime", blurb: "Actionable, undated." },
  { id: "Someday", label: "Someday", blurb: "Parked until you activate it." },
];

type Buckets = Record<Bucket, TaskView[]>;

const EMPTY_BUCKETS: Buckets = { Today: [], Upcoming: [], Anytime: [], Someday: [] };

const PRIORITY_LABEL: Record<number, string> = { 1: "Low", 2: "Medium", 3: "High" };

export function TasksView(): React.JSX.Element {
  const [buckets, setBuckets] = useState<Buckets>(EMPTY_BUCKETS);
  const [draft, setDraft] = useState<string>("");
  const [error, setError] = useState<string>("");
  const busy = useRef<boolean>(false);

  const refresh = useCallback(async (): Promise<void> => {
    try {
      const [today, upcoming, anytime, someday] = await Promise.all([
        api.tasksBucket("Today"),
        api.tasksBucket("Upcoming"),
        api.tasksBucket("Anytime"),
        api.tasksBucket("Someday"),
      ]);
      setBuckets({ Today: today, Upcoming: upcoming, Anytime: anytime, Someday: someday });
    } catch (e: unknown) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Re-read buckets whenever a task changes anywhere (capture, meetings, here).
  useEffect(() => {
    const unlisten = onAppEvent((ev) => {
      if (ev.type === "TaskChanged") void refresh();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refresh]);

  const create = useCallback(async (): Promise<void> => {
    const title = draft.trim();
    if (!title || busy.current) return;
    busy.current = true;
    try {
      await api.tasksCreate(title);
      setDraft("");
      await refresh();
    } catch (e: unknown) {
      setError(String(e));
    } finally {
      busy.current = false;
    }
  }, [draft, refresh]);

  const toggle = useCallback(
    async (task: TaskView): Promise<void> => {
      const next = task.status === "completed" ? "open" : "completed";
      try {
        await api.tasksSetStatus(task.id, next);
        await refresh();
      } catch (e: unknown) {
        setError(String(e));
      }
    },
    [refresh],
  );

  const total = BUCKETS.reduce((n, b) => n + buckets[b.id].length, 0);

  return (
    <div className="tasks">
      <div className="tasks-inner">
        <header className="tasks-head">
          <h1 className="tasks-h1">Tasks</h1>
          <span className="tasks-count">{total} open</span>
        </header>

        <form
          className="tasks-add"
          onSubmit={(e) => {
            e.preventDefault();
            void create();
          }}
        >
          <input
            className="tasks-add-input"
            placeholder="Add a task and press Enter"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            aria-label="New task"
          />
          <button type="submit" className="btn btn-accent" disabled={!draft.trim()}>
            Add
          </button>
        </form>

        {error && (
          <div className="error-banner" role="alert">
            {error}
          </div>
        )}

        {BUCKETS.map((b) => (
          <section key={b.id} className="tasks-bucket">
            <div className="tasks-bucket-head">
              <span className="tasks-bucket-title">{b.label}</span>
              <span className="tasks-bucket-count">{buckets[b.id].length}</span>
            </div>
            {buckets[b.id].length === 0 ? (
              <p className="tasks-empty">{b.blurb}</p>
            ) : (
              <ul className="tasks-list">
                {buckets[b.id].map((t) => (
                  <li key={t.id} className={`tasks-item${t.status === "completed" ? " done" : ""}`}>
                    <button
                      type="button"
                      className={`tasks-check${t.status === "completed" ? " on" : ""}`}
                      onClick={() => void toggle(t)}
                      aria-pressed={t.status === "completed"}
                      aria-label={t.status === "completed" ? "Mark open" : "Complete task"}
                    >
                      {t.status === "completed" ? "✓" : ""}
                    </button>
                    <span className="tasks-item-body">
                      <span className="tasks-item-title">{t.title ?? "Untitled task"}</span>
                      <span className="tasks-item-meta">
                        {t.priority > 0 && (
                          <span className={`tasks-flag p${t.priority}`}>
                            {PRIORITY_LABEL[t.priority] ?? "Priority"}
                          </span>
                        )}
                        {t.start_on && <span className="tasks-chip">Starts {t.start_on}</span>}
                        {t.deadline_on && <span className="tasks-chip due">Due {t.deadline_on}</span>}
                      </span>
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </section>
        ))}
      </div>
    </div>
  );
}
