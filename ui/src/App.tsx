/**
 * Casual Note — WebView shell root.
 *
 * The WebView is a thin JSON/event view (Architecture §13.1 decision #1): it never
 * sees SQL, raw filesystem paths, or PCM. It talks to the Rust core only through
 * `invoke(cmd, args)` (HLD §6) and reconciles state from `AppEvent`s (HLD §7).
 *
 * M0 Notes experience: a two-pane view — the notebook list on the left, a Tiptap
 * editor with debounced autosave on the right — plus intent-routed quick capture.
 * When run outside Tauri (`pnpm dev` in a browser) the IPC layer serves an
 * in-memory sample store so the whole app renders for preview.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { Editor } from "./features/editor/Editor";
import { NoteList } from "./features/notebooks/NoteList";
import { QuickCapture } from "./features/quick-capture/QuickCapture";
import { api, isTauri, onAppEvent, type NoteSummary } from "./lib/api";

const NOTE_EVENTS = new Set(["NoteSaved", "NoteProjected"]);

export function App(): React.JSX.Element {
  const [notes, setNotes] = useState<NoteSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [error, setError] = useState<string>("");
  const booted = useRef<boolean>(false);

  const refreshNotes = useCallback(async (): Promise<NoteSummary[]> => {
    const list = await api.notesList();
    setNotes(list);
    return list;
  }, []);

  const select = useCallback((id: string): void => {
    setSelectedId(id);
  }, []);

  // Boot: load the note list and open the most recent (or create the first note).
  useEffect(() => {
    if (booted.current) return;
    booted.current = true;
    void (async () => {
      try {
        const list = await refreshNotes();
        const first = list[0]?.id ?? (await api.notesCreate());
        if (!list[0]) await refreshNotes();
        select(first);
      } catch (e: unknown) {
        setError(String(e));
      }
    })();
  }, [refreshNotes, select]);

  // Reconcile from the single core→WebView event channel (HLD §7): any note
  // mutation re-fetches the summary list so titles/order stay live.
  useEffect(() => {
    const unlisten = onAppEvent((ev) => {
      if (NOTE_EVENTS.has(ev.type)) void refreshNotes().catch(() => undefined);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refreshNotes]);

  const createNote = useCallback(async (): Promise<void> => {
    try {
      const id = await api.notesCreate();
      await refreshNotes();
      select(id);
    } catch (e: unknown) {
      setError(String(e));
    }
  }, [refreshNotes, select]);

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark">Casual Note</span>
          <span className="brand-sub">Notes</span>
        </div>
        <QuickCapture
          onCaptured={(r) => {
            void refreshNotes().catch(() => undefined);
            if (r.entity_ref.kind === "note") select(r.entity_ref.id);
          }}
        />
        <span className="mode-pill">{isTauri ? "Local store" : "Preview (sample data)"}</span>
      </header>

      {error && (
        <div className="error-banner" role="alert">
          {error}
        </div>
      )}

      <div className="workspace">
        <NoteList
          notes={notes}
          selectedId={selectedId}
          onSelect={select}
          onCreate={() => void createNote()}
        />
        {selectedId ? (
          <Editor key={selectedId} noteId={selectedId} />
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
    </div>
  );
}
