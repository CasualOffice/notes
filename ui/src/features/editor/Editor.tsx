/**
 * The note editor (Feature Specs §1; HLD §8.1). A Tiptap editor with the full
 * Casual Note node/mark set, a "/" slash menu, `[[…` note autocomplete, clickable
 * wikilinks, notebook assignment, and Markdown import/export — all over the typed
 * IPC surface (`invoke` only; never SQL/FS).
 *
 * `doc_json` is the source of truth. On save we stamp any missing `blockId`s
 * client-side (so ids stay stable across edits), serialize `editor.getJSON()`, and
 * hand it to the core, which projects blocks / links / FTS Rust-side and returns
 * the new version token. Autosave is debounced; pending edits also flush when the
 * note is switched away, so a keystroke is never dropped on the floor.
 */
import { EditorContent, useEditor, type Editor as TiptapEditor } from "@tiptap/react";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  api,
  type NotebookNode,
  type NoteSummary,
  type NoteView,
} from "../../lib/api";
import { buildExtensions } from "./extensions";
import { SlashMenu } from "./SlashMenu";
import { useCompletion } from "./useCompletion";
import { WikilinkMenu } from "./WikilinkMenu";

const AUTOSAVE_MS = 700;

type Status = "idle" | "loading" | "edited" | "saving" | "saved" | "error";

function label(status: Status, version: number): string {
  switch (status) {
    case "loading":
      return "loading";
    case "edited":
      return "unsaved changes";
    case "saving":
      return "saving";
    case "saved":
      return `saved · v${version}`;
    case "error":
      return "save failed";
    default:
      return version ? `v${version}` : "";
  }
}

function genBlockId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) return crypto.randomUUID();
  return `b-${Math.random().toString(36).slice(2)}${Date.now().toString(36)}`;
}

/** Stamp a `blockId` on any top-level block that lacks one, in one history-free tx. */
function ensureBlockIds(editor: TiptapEditor): void {
  const stamp: number[] = [];
  editor.state.doc.forEach((node, offset) => {
    if (node.isBlock && "blockId" in node.attrs && !node.attrs["blockId"]) {
      stamp.push(offset);
    }
  });
  if (stamp.length === 0) return;
  editor
    .chain()
    .command(({ tr }) => {
      for (const pos of stamp) {
        const node = tr.doc.nodeAt(pos);
        if (node && "blockId" in node.attrs && !node.attrs["blockId"]) {
          tr.setNodeAttribute(pos, "blockId", genBlockId());
        }
      }
      tr.setMeta("blockIdStamp", true);
      tr.setMeta("addToHistory", false);
      return true;
    })
    .run();
}

/** Flatten the notebook forest into indented options for the move picker. */
function flattenNotebooks(nodes: NotebookNode[], depth = 0): { id: string; label: string }[] {
  const out: { id: string; label: string }[] = [];
  for (const n of nodes) {
    out.push({ id: n.id, label: `${"  ".repeat(depth)}${n.name ?? "Untitled notebook"}` });
    out.push(...flattenNotebooks(n.children, depth + 1));
  }
  return out;
}

interface Props {
  noteId: string;
  notes: NoteSummary[];
  notebooks: NotebookNode[];
  onOpenNote: (id: string) => void;
  onOpenWikilink: (target: string, targetId: string | null) => void;
  onChanged: () => void;
}

export function Editor({
  noteId,
  notes,
  notebooks,
  onOpenNote,
  onOpenWikilink,
  onChanged,
}: Props): React.JSX.Element {
  const [status, setStatus] = useState<Status>("loading");
  const [version, setVersion] = useState<number>(0);
  const [noteView, setNoteView] = useState<NoteView | null>(null);

  const versionRef = useRef<number>(0);
  const loadingRef = useRef<boolean>(true);
  const dirtyRef = useRef<boolean>(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const extensions = useMemo(() => buildExtensions(), []);

  const editor = useEditor({
    extensions,
    content: "<p></p>",
    onUpdate: ({ transaction }) => {
      if (loadingRef.current) return;
      if (transaction.getMeta("blockIdStamp")) return; // id-only stamp, not a user edit
      dirtyRef.current = true;
      setStatus("edited");
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => void flush(), AUTOSAVE_MS);
    },
  });

  const flush = async (): Promise<void> => {
    if (!editor || loadingRef.current || !dirtyRef.current) return;
    ensureBlockIds(editor);
    const docJson = JSON.stringify(editor.getJSON());
    dirtyRef.current = false;
    setStatus("saving");
    try {
      const res = await api.notesSave(noteId, docJson, versionRef.current);
      versionRef.current = res.version;
      setVersion(res.version);
      setStatus("saved");
    } catch (e: unknown) {
      dirtyRef.current = true;
      setStatus("error");
      console.error("autosave failed", e);
    }
  };

  // Load (or reload) the selected note. Programmatic setContent does not emit an
  // update, so it never spuriously triggers autosave. Pending edits from the
  // previous note are flushed on cleanup so no keystroke is lost on switch.
  useEffect(() => {
    if (!editor) return;
    let active = true;
    loadingRef.current = true;
    dirtyRef.current = false;
    setStatus("loading");
    void api
      .notesGet(noteId)
      .then((note) => {
        if (!active) return;
        setNoteView(note);
        versionRef.current = note.version;
        setVersion(note.version);
        try {
          editor.commands.setContent(JSON.parse(note.doc_json));
        } catch {
          editor.commands.setContent("<p></p>");
        }
        loadingRef.current = false;
        setStatus("idle");
      })
      .catch((e: unknown) => {
        if (active) {
          setStatus("error");
          console.error("note load failed", e);
        }
      });
    return () => {
      active = false;
      if (timerRef.current) clearTimeout(timerRef.current);
      void flush();
    };
    // flush intentionally excluded: it closes over the note being switched away.
  }, [noteId, editor]);

  // Flush pending edits before the window unloads (belt-and-braces durability).
  useEffect(() => {
    const onBeforeUnload = (): void => {
      if (timerRef.current) clearTimeout(timerRef.current);
      void flush();
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    return () => window.removeEventListener("beforeunload", onBeforeUnload);
  });

  const { trigger, dismiss } = useCompletion(editor);

  const onEditorClick = (e: React.MouseEvent<HTMLDivElement>): void => {
    const el = (e.target as HTMLElement).closest(".cn-wikilink");
    if (!el) return;
    e.preventDefault();
    const target = el.getAttribute("data-target") ?? el.textContent ?? "";
    const targetId = el.getAttribute("data-target-id");
    onOpenWikilink(target, targetId);
  };

  const moveToNotebook = (value: string): void => {
    const notebookId = value === "" ? null : value;
    void api
      .notesMove(noteId, notebookId)
      .then((updated) => {
        setNoteView(updated);
        onChanged();
      })
      .catch((err: unknown) => console.error("move failed", err));
  };

  const exportMarkdown = (): void => {
    void api
      .notesExportMarkdown(noteId)
      .then((md) => {
        const name = `${(noteView?.title ?? "note").replace(/[^\w-]+/g, "-").slice(0, 40) || "note"}.md`;
        const url = URL.createObjectURL(new Blob([md], { type: "text/markdown" }));
        const a = document.createElement("a");
        a.href = url;
        a.download = name;
        a.click();
        URL.revokeObjectURL(url);
      })
      .catch((err: unknown) => console.error("export failed", err));
  };

  const importMarkdown = (file: File): void => {
    void file.text().then((md) => {
      void api
        .notesImportMarkdown(md, noteView?.notebook_id ?? null)
        .then((note) => {
          onChanged();
          onOpenNote(note.id);
        })
        .catch((err: unknown) => console.error("import failed", err));
    });
  };

  const notebookOptions = useMemo(() => flattenNotebooks(notebooks), [notebooks]);

  return (
    <div className="editor-pane">
      <div className="editor-inner">
        <div className="editor-toolbar">
          <select
            className="nb-picker"
            aria-label="Notebook"
            value={noteView?.notebook_id ?? ""}
            onChange={(e) => moveToNotebook(e.target.value)}
          >
            <option value="">No notebook</option>
            {notebookOptions.map((o) => (
              <option key={o.id} value={o.id}>
                {o.label}
              </option>
            ))}
          </select>
          <div className="toolbar-spacer" />
          <button type="button" className="btn btn-ghost" onClick={() => fileInputRef.current?.click()}>
            Import
          </button>
          <button type="button" className="btn btn-ghost" onClick={exportMarkdown}>
            Export
          </button>
          <span className="editor-status">{label(status, version)}</span>
          <input
            ref={fileInputRef}
            type="file"
            accept=".md,.markdown,text/markdown,text/plain"
            style={{ display: "none" }}
            onChange={(e) => {
              const file = e.target.files?.[0];
              if (file) importMarkdown(file);
              e.target.value = "";
            }}
          />
        </div>

        <div className="editor-doc" onClick={onEditorClick}>
          <EditorContent editor={editor} />
        </div>
      </div>

      {editor && trigger?.kind === "slash" && (
        <SlashMenu editor={editor} trigger={trigger} onClose={dismiss} />
      )}
      {editor && trigger?.kind === "wikilink" && (
        <WikilinkMenu
          editor={editor}
          trigger={trigger}
          notes={notes.filter((n) => n.id !== noteId)}
          onClose={dismiss}
          onLinked={(id) => {
            onChanged();
            void id;
          }}
        />
      )}
    </div>
  );
}
