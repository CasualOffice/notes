/**
 * The `[[…` autocomplete over existing notes. Selecting a note inserts a resolved
 * wikilink (carrying its `targetId`); the "Create" row makes a new note first and
 * links to it. Navigates with arrows, accepts with Enter/Tab, dismisses with Escape.
 */
import type { Editor } from "@tiptap/react";
import { useEffect, useRef, useState } from "react";
import { api, type NoteSummary } from "../../lib/api";
import type { Trigger } from "./useCompletion";

type Range = { from: number; to: number };

interface Props {
  editor: Editor;
  trigger: Trigger;
  notes: NoteSummary[];
  onClose: () => void;
  onLinked: (noteId: string) => void;
}

const MAX_RESULTS = 8;

function insertWikilink(editor: Editor, range: Range, title: string, targetId: string | null): void {
  const attrs: Record<string, unknown> = { target: title };
  if (targetId) attrs["targetId"] = targetId;
  editor
    .chain()
    .focus()
    .deleteRange(range)
    .insertContent([
      { type: "text", text: title, marks: [{ type: "wikilink", attrs }] },
      { type: "text", text: " " },
    ])
    .run();
}

export function WikilinkMenu({ editor, trigger, notes, onClose, onLinked }: Props): React.JSX.Element | null {
  const q = trigger.query.trim().toLowerCase();
  const matches = notes
    .filter((n) => (n.title ?? "").toLowerCase().includes(q))
    .slice(0, MAX_RESULTS);
  // One row per match, plus an optional "create" row when there's a query.
  const rowCount = matches.length + (q ? 1 : 0);

  const [active, setActive] = useState<number>(0);
  const activeRef = useRef<number>(0);
  activeRef.current = Math.min(active, Math.max(rowCount - 1, 0));

  const ctx = useRef({ editor, trigger, matches, q, onClose, onLinked });
  ctx.current = { editor, trigger, matches, q, onClose, onLinked };

  useEffect(() => {
    setActive(0);
  }, [trigger.query]);

  const acceptExisting = (n: NoteSummary): void => {
    insertWikilink(ctx.current.editor, ctx.current.trigger.range, n.title ?? "Untitled", n.id);
    ctx.current.onClose();
  };

  const acceptCreate = (): void => {
    const { editor: ed, trigger: tr, onClose: close, onLinked: linked } = ctx.current;
    const title = tr.query.trim();
    if (!title) return;
    const docJson = JSON.stringify({
      type: "doc",
      content: [{ type: "heading", attrs: { level: 1 }, content: [{ type: "text", text: title }] }],
    });
    void api
      .notesCreate(docJson)
      .then((id) => {
        insertWikilink(ed, tr.range, title, id);
        linked(id);
      })
      .catch(() => {
        // Fall back to an unresolved link so the keystroke is never lost.
        insertWikilink(ed, tr.range, title, null);
      })
      .finally(() => close());
  };

  const acceptRow = (index: number): void => {
    const { matches: list } = ctx.current;
    if (index < list.length) {
      const n = list[index];
      if (n) acceptExisting(n);
    } else {
      acceptCreate();
    }
  };

  useEffect(() => {
    const onKey = (ev: KeyboardEvent): void => {
      const total = ctx.current.matches.length + (ctx.current.q ? 1 : 0);
      if (total === 0) return;
      const handle = (): void => {
        ev.preventDefault();
        ev.stopPropagation();
      };
      if (ev.key === "ArrowDown") {
        handle();
        setActive((i) => (i + 1) % total);
      } else if (ev.key === "ArrowUp") {
        handle();
        setActive((i) => (i - 1 + total) % total);
      } else if (ev.key === "Enter" || ev.key === "Tab") {
        handle();
        acceptRow(activeRef.current);
      } else if (ev.key === "Escape") {
        handle();
        ctx.current.onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  if (rowCount === 0) return null;

  return (
    <div
      className="cn-menu"
      style={{ left: trigger.coords.left, top: trigger.coords.bottom + 4 }}
      role="listbox"
      aria-label="Link to note"
    >
      {matches.map((n, i) => (
        <button
          key={n.id}
          type="button"
          role="option"
          aria-selected={i === activeRef.current}
          className={`cn-menu-item${i === activeRef.current ? " active" : ""}`}
          onMouseDown={(e) => e.preventDefault()}
          onMouseEnter={() => setActive(i)}
          onClick={() => acceptExisting(n)}
        >
          <span className="cn-menu-label">{n.title ?? "Untitled"}</span>
          <span className="cn-menu-hint">note</span>
        </button>
      ))}
      {q && (
        <button
          type="button"
          role="option"
          aria-selected={activeRef.current === matches.length}
          className={`cn-menu-item${activeRef.current === matches.length ? " active" : ""}`}
          onMouseDown={(e) => e.preventDefault()}
          onMouseEnter={() => setActive(matches.length)}
          onClick={() => acceptCreate()}
        >
          <span className="cn-menu-label">Create “{trigger.query.trim()}”</span>
          <span className="cn-menu-hint">new note</span>
        </button>
      )}
    </div>
  );
}
