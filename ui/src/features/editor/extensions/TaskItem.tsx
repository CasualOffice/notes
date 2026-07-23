/**
 * `taskItem` — a to-do block with a leading checkbox (Feature Specs §1.1).
 *
 * A standalone block (not wrapped in a list) whose `checked` attribute round-trips
 * to Markdown as `- [ ]` / `- [x]` (see `notes::markdown`). The checkbox itself is
 * non-editable chrome; the label holds the inline content.
 */
import {
  Node,
  NodeViewContent,
  NodeViewWrapper,
  ReactNodeViewRenderer,
  mergeAttributes,
  type ReactNodeViewProps,
} from "@tiptap/react";

function TaskItemView({ node, updateAttributes }: ReactNodeViewProps): React.JSX.Element {
  const checked = Boolean(node.attrs["checked"]);
  return (
    <NodeViewWrapper as="div" className={`cn-task${checked ? " checked" : ""}`}>
      <span className="cn-task-box" contentEditable={false}>
        <input
          type="checkbox"
          checked={checked}
          aria-label="Toggle task"
          onChange={(e) => updateAttributes({ checked: e.target.checked })}
        />
      </span>
      <NodeViewContent as="div" className="cn-task-body" />
    </NodeViewWrapper>
  );
}

export const TaskItem = Node.create({
  name: "taskItem",
  group: "block",
  content: "inline*",
  defining: true,

  addAttributes() {
    return {
      checked: {
        default: false,
        keepOnSplit: false,
        parseHTML: (element) => element.getAttribute("data-checked") === "true",
        renderHTML: (attributes) => ({ "data-checked": attributes["checked"] ? "true" : "false" }),
      },
    };
  },

  parseHTML() {
    return [{ tag: 'li[data-type="task-item"]' }, { tag: 'div[data-type="task-item"]' }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes(HTMLAttributes, { "data-type": "task-item" }), 0];
  },

  addNodeView() {
    return ReactNodeViewRenderer(TaskItemView);
  },
});
