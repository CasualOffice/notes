/**
 * The full editor extension set for Casual Note (M1).
 *
 * StarterKit supplies the common nodes/marks (paragraph, headings, lists, code
 * block, blockquote, bold/italic/strike/code, history). On top we register the
 * custom nodes and marks whose type names and attributes match the Rust schema
 * exactly, so every construct round-trips through `doc_json` and Markdown.
 */
import StarterKit from "@tiptap/starter-kit";
import type { Extensions } from "@tiptap/react";
import { BlockId } from "./blockId";
import { Callout } from "./Callout";
import { Mention, Tag, Wikilink } from "./marks";
import { Table, TableCell, TableHeader, TableRow } from "./table";
import { TaskItem } from "./TaskItem";

export function buildExtensions(): Extensions {
  return [
    StarterKit.configure({ heading: { levels: [1, 2, 3] } }),
    BlockId,
    TaskItem,
    Callout,
    Table,
    TableRow,
    TableHeader,
    TableCell,
    Wikilink,
    Tag,
    Mention,
  ];
}

export { emptyTable } from "./table";
