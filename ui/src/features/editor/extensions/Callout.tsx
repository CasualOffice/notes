/**
 * `callout` — an admonition block (`> [!type]` in Markdown; see `notes::markdown`).
 *
 * Holds block content (paragraphs) and a `type` attribute that selects the accent
 * (note / info / tip / warning). The type picker is non-editable chrome.
 */
import {
  Node,
  NodeViewContent,
  NodeViewWrapper,
  ReactNodeViewRenderer,
  mergeAttributes,
  type ReactNodeViewProps,
} from "@tiptap/react";

const CALLOUT_TYPES = ["note", "info", "tip", "warning"] as const;

function CalloutView({ node, updateAttributes }: ReactNodeViewProps): React.JSX.Element {
  const type = String(node.attrs["type"] ?? "note");
  return (
    <NodeViewWrapper as="div" className={`cn-callout cn-callout-${type}`} data-callout={type}>
      <div className="cn-callout-bar" contentEditable={false}>
        <select
          className="cn-callout-type"
          value={type}
          aria-label="Callout type"
          onChange={(e) => updateAttributes({ type: e.target.value })}
        >
          {CALLOUT_TYPES.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </select>
      </div>
      <NodeViewContent as="div" className="cn-callout-body" />
    </NodeViewWrapper>
  );
}

export const Callout = Node.create({
  name: "callout",
  group: "block",
  content: "block+",
  defining: true,

  addAttributes() {
    return {
      type: {
        default: "note",
        parseHTML: (element) => element.getAttribute("data-callout") ?? "note",
        renderHTML: (attributes) => ({ "data-callout": String(attributes["type"] ?? "note") }),
      },
    };
  },

  parseHTML() {
    return [{ tag: 'div[data-type="callout"]' }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes(HTMLAttributes, { "data-type": "callout" }), 0];
  },

  addNodeView() {
    return ReactNodeViewRenderer(CalloutView);
  },
});
