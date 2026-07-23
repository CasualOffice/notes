/**
 * Tiptap editor bound to the selected note, with debounced autosave (HLD §8.1).
 * `doc_json` is the source of truth: on save we serialize `editor.getJSON()` and
 * hand it to the core, which projects blocks / links / FTS Rust-side and returns
 * the new version token for optimistic concurrency. Title is derived server-side
 * from the body, so saving one note keeps the sidebar in sync via `NoteSaved`.
 */
import { EditorContent, useEditor } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import { useEffect, useRef, useState } from "react";
import { api } from "../../lib/api";

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

export function Editor({ noteId }: { noteId: string }): React.JSX.Element {
  const [status, setStatus] = useState<Status>("loading");
  const [version, setVersion] = useState<number>(0);

  // Refs keep the autosave closure current without re-creating the editor.
  const versionRef = useRef<number>(0);
  const loadingRef = useRef<boolean>(true);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const editor = useEditor({
    extensions: [StarterKit],
    content: "<p></p>",
    onUpdate: () => {
      if (loadingRef.current) return;
      setStatus("edited");
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => void flush(), AUTOSAVE_MS);
    },
  });

  const flush = async (): Promise<void> => {
    if (!editor || loadingRef.current) return;
    const docJson = JSON.stringify(editor.getJSON());
    setStatus("saving");
    try {
      const res = await api.notesSave(noteId, docJson, versionRef.current);
      versionRef.current = res.version;
      setVersion(res.version);
      setStatus("saved");
    } catch (e: unknown) {
      setStatus("error");
      console.error("autosave failed", e);
    }
  };

  // Load (or reload) the selected note. Programmatic setContent does not emit an
  // update, so it never spuriously triggers autosave.
  useEffect(() => {
    if (!editor) return;
    let active = true;
    loadingRef.current = true;
    setStatus("loading");
    api
      .notesGet(noteId)
      .then((note) => {
        if (!active) return;
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
    };
  }, [noteId, editor]);

  return (
    <div className="editor-pane">
      <div className="editor-inner">
        <div className="editor-status">{label(status, version)}</div>
        <EditorContent editor={editor} />
      </div>
    </div>
  );
}
