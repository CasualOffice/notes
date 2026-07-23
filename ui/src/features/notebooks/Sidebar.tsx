/**
 * Left sidebar for the Notes view: a pinned "Today" entry that opens the daily
 * note (`daily.get_or_create`), the notebook/folder tree, and the note list for
 * the selected notebook.
 */
import type { NotebookNode, NoteSummary } from "../../lib/api";
import { NoteList } from "./NoteList";
import { NotebookTree } from "./NotebookTree";

interface Props {
  notebooks: NotebookNode[];
  selectedNotebookId: string | null;
  onSelectNotebook: (id: string | null) => void;
  onCreateNotebook: (name: string, parentId: string | null) => void;
  notes: NoteSummary[];
  selectedNoteId: string | null;
  onSelectNote: (id: string) => void;
  onCreateNote: () => void;
  onOpenToday: () => void;
  todayActive: boolean;
}

export function Sidebar({
  notebooks,
  selectedNotebookId,
  onSelectNotebook,
  onCreateNotebook,
  notes,
  selectedNoteId,
  onSelectNote,
  onCreateNote,
  onOpenToday,
  todayActive,
}: Props): React.JSX.Element {
  return (
    <aside className="sidebar">
      <div className="sidebar-scroll">
        <button
          type="button"
          className={`today-item${todayActive ? " active" : ""}`}
          onClick={onOpenToday}
        >
          <span className="today-mark">Today</span>
          <span className="today-date">
            {new Date().toLocaleDateString(undefined, { month: "short", day: "numeric" })}
          </span>
        </button>

        <NotebookTree
          nodes={notebooks}
          selectedId={selectedNotebookId}
          onSelect={onSelectNotebook}
          onCreate={onCreateNotebook}
        />

        <NoteList
          notes={notes}
          selectedId={selectedNoteId}
          onSelect={onSelectNote}
          onCreate={onCreateNote}
        />
      </div>
    </aside>
  );
}
