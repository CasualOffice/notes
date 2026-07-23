/**
 * Detects the active inline completion trigger at the caret — a slash command
 * (`/…`) or an open wikilink (`[[…`) — and reports its query, the document range
 * to replace on accept, and the caret's screen coordinates for popup placement.
 *
 * Built entirely on the public editor API (selection + `coordsAtPos`); no
 * ProseMirror plugin is required, which keeps the editor's extension surface small.
 */
import type { Editor } from "@tiptap/react";
import { useEffect, useRef, useState } from "react";

export type TriggerKind = "slash" | "wikilink";

export interface Trigger {
  kind: TriggerKind;
  query: string;
  /** Inclusive-start / exclusive-end doc positions of the trigger text. */
  range: { from: number; to: number };
  coords: { left: number; top: number; bottom: number };
}

function detect(editor: Editor): Trigger | null {
  const { state, view } = editor;
  const { selection } = state;
  if (!selection.empty) return null;
  const { $from } = selection;
  const from = selection.from;
  const before = $from.parent.textBetween(0, $from.parentOffset, "\n", "￼");

  let coords: { left: number; top: number; bottom: number };
  try {
    const c = view.coordsAtPos(from);
    coords = { left: c.left, top: c.top, bottom: c.bottom };
  } catch {
    return null;
  }

  // Open wikilink: `[[` with no closing `]]` yet.
  const wiki = /\[\[([^[\]\n]*)$/.exec(before);
  if (wiki) {
    const query = wiki[1] ?? "";
    return { kind: "wikilink", query, range: { from: from - (query.length + 2), to: from }, coords };
  }

  // Slash command: `/word` at a line start or after whitespace, in a paragraph.
  if ($from.parent.type.name === "paragraph") {
    const slash = /(?:^|\s)\/([a-zA-Z]*)$/.exec(before);
    if (slash) {
      const query = slash[1] ?? "";
      return { kind: "slash", query, range: { from: from - (query.length + 1), to: from }, coords };
    }
  }
  return null;
}

export function useCompletion(editor: Editor | null): { trigger: Trigger | null; dismiss: () => void } {
  const [trigger, setTrigger] = useState<Trigger | null>(null);
  // Suppression key: while set, a trigger at this exact spot stays hidden (Escape).
  const suppressed = useRef<string | null>(null);

  useEffect(() => {
    if (!editor) return;
    const recompute = (): void => {
      const next = detect(editor);
      // Any change in the query/spot clears a prior Escape suppression.
      const spot = next ? `${next.kind}:${next.range.from}` : null;
      if (suppressed.current && suppressed.current !== spot) suppressed.current = null;
      if (next && suppressed.current === spot) {
        setTrigger(null);
        return;
      }
      setTrigger(next);
    };
    editor.on("transaction", recompute);
    editor.on("selectionUpdate", recompute);
    recompute();
    return () => {
      editor.off("transaction", recompute);
      editor.off("selectionUpdate", recompute);
    };
  }, [editor]);

  const dismiss = (): void => {
    if (trigger) suppressed.current = `${trigger.kind}:${trigger.range.from}`;
    setTrigger(null);
  };

  return { trigger, dismiss };
}
