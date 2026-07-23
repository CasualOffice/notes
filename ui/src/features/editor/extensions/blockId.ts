/**
 * A global `blockId` attribute attached to every block-level node.
 *
 * The Rust projection mints a stable `blockId` for each leaf block on save
 * (`notes::projection::ensure_block_ids`) and persists it into `doc_json`. For
 * those ids to survive an edit round-trip the editor schema must *know* the
 * attribute — otherwise ProseMirror strips it on load and the core re-mints a
 * fresh id on the next save (blockId churn). Declaring it here keeps ids stable:
 * loaded ids are preserved in `getJSON()`, and the editor stamps any still-missing
 * ids client-side before autosave (see `Editor.tsx`).
 */
import { Extension } from "@tiptap/react";

/** Block node types that carry a stable `blockId`. */
export const BLOCK_ID_TYPES = [
  "paragraph",
  "heading",
  "blockquote",
  "codeBlock",
  "bulletList",
  "orderedList",
  "listItem",
  "taskItem",
  "callout",
  "table",
  "tableRow",
  "tableHeader",
  "tableCell",
  "horizontalRule",
  "image",
] as const;

export const BlockId = Extension.create({
  name: "blockId",

  addGlobalAttributes() {
    return [
      {
        types: [...BLOCK_ID_TYPES],
        attributes: {
          blockId: {
            default: null,
            keepOnSplit: false,
            parseHTML: (element) => element.getAttribute("data-block-id"),
            renderHTML: (attributes) => {
              const id = attributes["blockId"];
              return typeof id === "string" && id ? { "data-block-id": id } : {};
            },
          },
        },
      },
    ];
  },
});
