/**
 * Notebook/folder tree (Feature Specs §2; `notebooks.list` / `notebooks.create`).
 * A recursive, collapsible tree with an "All notes" root and inline creation of
 * root and child notebooks. Selecting a node filters the note list to it.
 */
import { useState } from "react";
import type { NotebookNode } from "../../lib/api";

interface Props {
  nodes: NotebookNode[];
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  onCreate: (name: string, parentId: string | null) => void;
}

/** `undefined` = no add form open; `null` = adding a root; string = adding a child. */
type Adding = string | null | undefined;

export function NotebookTree({ nodes, selectedId, onSelect, onCreate }: Props): React.JSX.Element {
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [adding, setAdding] = useState<Adding>(undefined);
  const [draft, setDraft] = useState<string>("");

  const toggle = (id: string): void =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const openAdd = (parentId: string | null): void => {
    if (parentId) setExpanded((prev) => new Set(prev).add(parentId));
    setAdding(parentId);
    setDraft("");
  };

  const commit = (): void => {
    const name = draft.trim();
    if (name) onCreate(name, adding ?? null);
    setAdding(undefined);
    setDraft("");
  };

  const addForm = (parentId: string | null): React.JSX.Element => (
    <div className="nb-add" style={{ paddingLeft: (parentId ? depthOf(parentId, nodes) + 1 : 0) * 14 + 8 }}>
      <input
        autoFocus
        value={draft}
        placeholder="Notebook name"
        aria-label="New notebook name"
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape") setAdding(undefined);
        }}
        onBlur={() => setAdding(undefined)}
      />
    </div>
  );

  const renderNode = (node: NotebookNode, depth: number): React.JSX.Element => {
    const hasChildren = node.children.length > 0;
    const isOpen = expanded.has(node.id);
    return (
      <li key={node.id}>
        <div className={`nb-row${node.id === selectedId ? " active" : ""}`} style={{ paddingLeft: depth * 14 + 6 }}>
          <button
            type="button"
            className="nb-caret"
            aria-label={isOpen ? "Collapse" : "Expand"}
            onClick={() => hasChildren && toggle(node.id)}
            style={{ visibility: hasChildren ? "visible" : "hidden" }}
          >
            {isOpen ? "▾" : "▸"}
          </button>
          <button type="button" className="nb-name" onClick={() => onSelect(node.id)}>
            {node.name ?? "Untitled notebook"}
          </button>
          <button
            type="button"
            className="nb-add-btn"
            aria-label="New sub-notebook"
            title="New sub-notebook"
            onClick={() => openAdd(node.id)}
          >
            +
          </button>
        </div>
        {adding === node.id && addForm(node.id)}
        {hasChildren && isOpen && (
          <ul className="nb-children">{node.children.map((c) => renderNode(c, depth + 1))}</ul>
        )}
      </li>
    );
  };

  return (
    <div className="nb-tree">
      <div className="sidebar-head">
        <span className="sidebar-title">Notebooks</span>
        <button
          type="button"
          className="btn btn-ghost"
          aria-label="New notebook"
          title="New notebook"
          onClick={() => openAdd(null)}
        >
          +
        </button>
      </div>
      <ul className="nb-list">
        <li>
          <div className={`nb-row${selectedId === null ? " active" : ""}`} style={{ paddingLeft: 6 }}>
            <span className="nb-caret" style={{ visibility: "hidden" }}>
              ▸
            </span>
            <button type="button" className="nb-name" onClick={() => onSelect(null)}>
              All notes
            </button>
          </div>
        </li>
        {nodes.map((n) => renderNode(n, 0))}
      </ul>
      {adding === null && addForm(null)}
    </div>
  );
}

/** Depth of a notebook in the forest (for indenting its inline add form). */
function depthOf(id: string, nodes: NotebookNode[], depth = 0): number {
  for (const n of nodes) {
    if (n.id === id) return depth;
    const d = depthOf(id, n.children, depth + 1);
    if (d >= 0) return d;
  }
  return -1;
}
