/**
 * Inline semantic marks: `[[wikilink]]`, `#tag`, `@mention`.
 *
 * These are marks (not decorations) so they persist in `doc_json` and drive the
 * Rust link projection. Attribute names match `notes::markdown` / `notes::validate`
 * exactly: wikilink→`target`/`targetId`/`alias`, tag→`name`, mention→`label`.
 *
 * Input rules apply the mark as you type: `[[Title]]` on the closing bracket, and
 * `#tag` / `@name` incrementally as the token grows. The `[[…` autocomplete over
 * existing notes is driven separately from the editor component, which inserts a
 * fully-resolved wikilink (with `targetId`) directly.
 */
import { Mark, markInputRule, mergeAttributes } from "@tiptap/react";

export const Wikilink = Mark.create({
  name: "wikilink",
  inclusive: false,

  addAttributes() {
    return {
      target: {
        default: null,
        parseHTML: (el) => el.getAttribute("data-target"),
        renderHTML: (attrs) => (attrs["target"] ? { "data-target": String(attrs["target"]) } : {}),
      },
      targetId: {
        default: null,
        parseHTML: (el) => el.getAttribute("data-target-id"),
        renderHTML: (attrs) =>
          attrs["targetId"] ? { "data-target-id": String(attrs["targetId"]) } : {},
      },
      alias: {
        default: null,
        parseHTML: (el) => el.getAttribute("data-alias"),
        renderHTML: (attrs) => (attrs["alias"] ? { "data-alias": String(attrs["alias"]) } : {}),
      },
    };
  },

  parseHTML() {
    return [{ tag: "a[data-wikilink]" }];
  },

  renderHTML({ HTMLAttributes }) {
    return [
      "a",
      mergeAttributes(HTMLAttributes, { "data-wikilink": "", class: "cn-wikilink" }),
      0,
    ];
  },

  addInputRules() {
    return [
      markInputRule({
        find: /\[\[([^\]\n]+)\]\]$/,
        type: this.type,
        getAttributes: (match) => ({ target: match[1] ?? "" }),
      }),
    ];
  },
});

export const Tag = Mark.create({
  name: "tag",
  inclusive: false,

  addAttributes() {
    return {
      name: {
        default: null,
        parseHTML: (el) => el.getAttribute("data-tag"),
        renderHTML: (attrs) => (attrs["name"] ? { "data-tag": String(attrs["name"]) } : {}),
      },
    };
  },

  parseHTML() {
    return [{ tag: "span[data-tag]" }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["span", mergeAttributes(HTMLAttributes, { class: "cn-tag" }), 0];
  },

  addInputRules() {
    return [
      markInputRule({
        find: /(?:^|\s)(#[a-zA-Z0-9_/-]+)$/,
        type: this.type,
        getAttributes: (match) => ({ name: (match[1] ?? "").slice(1) }),
      }),
    ];
  },
});

export const Mention = Mark.create({
  name: "mention",
  inclusive: false,

  addAttributes() {
    return {
      label: {
        default: null,
        parseHTML: (el) => el.getAttribute("data-mention"),
        renderHTML: (attrs) => (attrs["label"] ? { "data-mention": String(attrs["label"]) } : {}),
      },
    };
  },

  parseHTML() {
    return [{ tag: "span[data-mention]" }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["span", mergeAttributes(HTMLAttributes, { class: "cn-mention" }), 0];
  },

  addInputRules() {
    return [
      markInputRule({
        find: /(?:^|\s)(@[a-zA-Z0-9_/-]+)$/,
        type: this.type,
        getAttributes: (match) => ({ label: (match[1] ?? "").slice(1) }),
      }),
    ];
  },
});
