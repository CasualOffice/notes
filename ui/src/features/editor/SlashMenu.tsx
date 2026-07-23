/**
 * The "/" slash menu: insert a block by name. Filters as you type after the slash,
 * navigates with arrows, accepts with Enter/Tab, dismisses with Escape. Each command
 * first deletes the trigger text, then applies its transform on the current block.
 */
import type { Editor } from "@tiptap/react";
import { useEffect, useRef, useState } from "react";
import { emptyTable } from "./extensions";
import type { Trigger } from "./useCompletion";

type Range = { from: number; to: number };

interface Command {
  label: string;
  hint: string;
  keywords: string;
  run: (editor: Editor, range: Range) => void;
}

const COMMANDS: Command[] = [
  {
    label: "Heading 1",
    hint: "Large section title",
    keywords: "h1 heading title",
    run: (e, r) => e.chain().focus().deleteRange(r).setNode("heading", { level: 1 }).run(),
  },
  {
    label: "Heading 2",
    hint: "Medium heading",
    keywords: "h2 heading subtitle",
    run: (e, r) => e.chain().focus().deleteRange(r).setNode("heading", { level: 2 }).run(),
  },
  {
    label: "Heading 3",
    hint: "Small heading",
    keywords: "h3 heading",
    run: (e, r) => e.chain().focus().deleteRange(r).setNode("heading", { level: 3 }).run(),
  },
  {
    label: "To-do",
    hint: "Checkable task item",
    keywords: "task todo checkbox check",
    run: (e, r) => e.chain().focus().deleteRange(r).setNode("taskItem", { checked: false }).run(),
  },
  {
    label: "Bulleted list",
    hint: "Unordered list",
    keywords: "bullet list ul unordered",
    run: (e, r) => e.chain().focus().deleteRange(r).toggleBulletList().run(),
  },
  {
    label: "Numbered list",
    hint: "Ordered list",
    keywords: "number ordered ol list",
    run: (e, r) => e.chain().focus().deleteRange(r).toggleOrderedList().run(),
  },
  {
    label: "Callout",
    hint: "Highlighted admonition",
    keywords: "callout note info warning tip admonition",
    run: (e, r) => e.chain().focus().deleteRange(r).wrapIn("callout").run(),
  },
  {
    label: "Quote",
    hint: "Block quotation",
    keywords: "quote blockquote",
    run: (e, r) => e.chain().focus().deleteRange(r).toggleBlockquote().run(),
  },
  {
    label: "Code block",
    hint: "Monospaced code",
    keywords: "code pre fenced",
    run: (e, r) => e.chain().focus().deleteRange(r).toggleCodeBlock().run(),
  },
  {
    label: "Table",
    hint: "2×2 table",
    keywords: "table grid",
    run: (e, r) => e.chain().focus().deleteRange(r).insertContent(emptyTable()).run(),
  },
  {
    label: "Divider",
    hint: "Horizontal rule",
    keywords: "divider hr rule separator",
    run: (e, r) => e.chain().focus().deleteRange(r).setHorizontalRule().run(),
  },
];

interface Props {
  editor: Editor;
  trigger: Trigger;
  onClose: () => void;
}

export function SlashMenu({ editor, trigger, onClose }: Props): React.JSX.Element | null {
  const q = trigger.query.toLowerCase();
  const items = COMMANDS.filter(
    (c) => q === "" || c.label.toLowerCase().includes(q) || c.keywords.includes(q),
  );
  const [active, setActive] = useState<number>(0);
  const activeRef = useRef<number>(0);
  activeRef.current = Math.min(active, Math.max(items.length - 1, 0));

  // Latest render values the (mount-once) key handler reads through refs, so it
  // never acts on a stale trigger range while the query is still changing.
  const ctx = useRef({ editor, trigger, items, onClose });
  ctx.current = { editor, trigger, items, onClose };

  useEffect(() => {
    setActive(0);
  }, [trigger.query]);

  const accept = (cmd: Command | undefined): void => {
    if (!cmd) return;
    cmd.run(ctx.current.editor, ctx.current.trigger.range);
    ctx.current.onClose();
  };

  useEffect(() => {
    const onKey = (ev: KeyboardEvent): void => {
      const list = ctx.current.items;
      if (list.length === 0) return;
      const handle = (): void => {
        ev.preventDefault();
        ev.stopPropagation();
      };
      if (ev.key === "ArrowDown") {
        handle();
        setActive((i) => (i + 1) % list.length);
      } else if (ev.key === "ArrowUp") {
        handle();
        setActive((i) => (i - 1 + list.length) % list.length);
      } else if (ev.key === "Enter" || ev.key === "Tab") {
        handle();
        accept(list[activeRef.current]);
      } else if (ev.key === "Escape") {
        handle();
        ctx.current.onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  if (items.length === 0) return null;

  return (
    <div
      className="cn-menu"
      style={{ left: trigger.coords.left, top: trigger.coords.bottom + 4 }}
      role="listbox"
      aria-label="Insert block"
    >
      {items.map((c, i) => (
        <button
          key={c.label}
          type="button"
          role="option"
          aria-selected={i === activeRef.current}
          className={`cn-menu-item${i === activeRef.current ? " active" : ""}`}
          onMouseDown={(e) => e.preventDefault()}
          onMouseEnter={() => setActive(i)}
          onClick={() => accept(c)}
        >
          <span className="cn-menu-label">{c.label}</span>
          <span className="cn-menu-hint">{c.hint}</span>
        </button>
      ))}
    </div>
  );
}
