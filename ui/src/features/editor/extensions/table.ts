/**
 * A minimal GFM-shaped table: `table > tableRow > (tableHeader | tableCell)`.
 *
 * Deliberately lightweight (no `prosemirror-tables` dependency): cells hold inline
 * content and the node types match exactly what `notes::markdown::table_to_md`
 * expects, so tables round-trip to `| a | b |` and back. First row = header cells.
 */
import { Node, mergeAttributes, type JSONContent } from "@tiptap/react";

export const Table = Node.create({
  name: "table",
  group: "block",
  content: "tableRow+",
  isolating: true,

  parseHTML() {
    return [{ tag: "table" }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["table", mergeAttributes(HTMLAttributes, { class: "cn-table" }), ["tbody", 0]];
  },
});

export const TableRow = Node.create({
  name: "tableRow",
  content: "(tableHeader | tableCell)+",

  parseHTML() {
    return [{ tag: "tr" }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["tr", mergeAttributes(HTMLAttributes), 0];
  },
});

export const TableHeader = Node.create({
  name: "tableHeader",
  content: "inline*",
  isolating: true,

  parseHTML() {
    return [{ tag: "th" }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["th", mergeAttributes(HTMLAttributes), 0];
  },
});

export const TableCell = Node.create({
  name: "tableCell",
  content: "inline*",
  isolating: true,

  parseHTML() {
    return [{ tag: "td" }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["td", mergeAttributes(HTMLAttributes), 0];
  },
});

/** JSON for a 2×2 starter table (header row + one body row). */
export function emptyTable(): JSONContent {
  const headerCell = (t: string): JSONContent => ({
    type: "tableHeader",
    content: [{ type: "text", text: t }],
  });
  const bodyCell = (): JSONContent => ({ type: "tableCell" });
  return {
    type: "table",
    content: [
      { type: "tableRow", content: [headerCell("Column"), headerCell("Column")] },
      { type: "tableRow", content: [bodyCell(), bodyCell()] },
    ],
  };
}
