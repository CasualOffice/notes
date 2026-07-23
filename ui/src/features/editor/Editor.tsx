/**
 * Minimal but real Tiptap editor wired to `notes.save` (HLD §8.1). `doc_json` is the
 * source of truth: on save we serialize `editor.getJSON()` and hand it to the core,
 * which projects blocks / links / FTS Rust-side and returns the new version token
 * for optimistic concurrency.
 */
import { EditorContent, useEditor } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import { useEffect, useState } from "react";
import { api } from "../../lib/api";

export function Editor({ noteId }: { noteId: string }): React.JSX.Element {
  const [version, setVersion] = useState<number>(0);
  const [status, setStatus] = useState<string>("loading…");

  const editor = useEditor({
    extensions: [StarterKit],
    content: "<p></p>",
  });

  useEffect(() => {
    if (!editor) return;
    let active = true;
    api
      .notesGet(noteId)
      .then((note) => {
        if (!active) return;
        setVersion(note.version);
        try {
          editor.commands.setContent(JSON.parse(note.doc_json));
        } catch {
          /* keep the empty document if doc_json is unparseable */
        }
        setStatus(`v${note.version}`);
      })
      .catch((e: unknown) => {
        if (active) setStatus(`load failed: ${String(e)}`);
      });
    return () => {
      active = false;
    };
  }, [noteId, editor]);

  const save = async (): Promise<void> => {
    if (!editor) return;
    const docJson = JSON.stringify(editor.getJSON());
    try {
      const res = await api.notesSave(noteId, docJson, version);
      setVersion(res.version);
      setStatus(`saved v${res.version} · ${res.changed_block_ids.length} blocks`);
    } catch (e: unknown) {
      setStatus(`save failed: ${String(e)}`);
    }
  };

  return (
    <section style={{ border: "1px solid #ccc", borderRadius: 8, padding: 12 }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <h2 style={{ margin: 0, fontSize: 16 }}>Note</h2>
        <span style={{ fontSize: 12, opacity: 0.7 }}>{status}</span>
      </div>
      <div style={{ minHeight: 160, marginTop: 8 }}>
        <EditorContent editor={editor} />
      </div>
      <button type="button" onClick={() => void save()} style={{ marginTop: 8 }}>
        Save
      </button>
    </section>
  );
}
