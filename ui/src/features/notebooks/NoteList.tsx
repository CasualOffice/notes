/**
 * The notebook list — left pane of the Notes view. Lists every note (`notes.list`),
 * creates new ones (`notes.create`), and lets the user select one to open. Titles
 * are the server-derived summary titles; a null title renders as "Untitled".
 */
import type { NoteSummary } from "../../lib/api";

function relativeTime(ms: number): string {
  const diff = Date.now() - ms;
  const mins = Math.round(diff / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d ago`;
  return new Date(ms).toLocaleDateString();
}

interface Props {
  notes: NoteSummary[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onCreate: () => void;
}

export function NoteList({ notes, selectedId, onSelect, onCreate }: Props): React.JSX.Element {
  return (
    <aside className="sidebar">
      <div className="sidebar-head">
        <span className="sidebar-title">Notes</span>
        <button type="button" className="btn btn-ghost" onClick={onCreate}>
          New note
        </button>
      </div>
      {notes.length === 0 ? (
        <p className="empty">No notes yet. Create your first one.</p>
      ) : (
        <ul className="note-list">
          {notes.map((n) => (
            <li key={n.id}>
              <button
                type="button"
                className={`note-item${n.id === selectedId ? " active" : ""}`}
                onClick={() => onSelect(n.id)}
                aria-current={n.id === selectedId}
              >
                <div className={`note-item-title${n.title ? "" : " untitled"}`}>
                  {n.title ?? "Untitled"}
                </div>
                <div className="note-item-meta">{relativeTime(n.updated_at)}</div>
              </button>
            </li>
          ))}
        </ul>
      )}
    </aside>
  );
}
