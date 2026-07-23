import { describe, expect, it } from "vitest";
import { mockCore } from "./mock";
import type { BacklinkRef, Note, NotebookNode, NoteSummary } from "./api";

const call = <T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> =>
  mockCore.invoke<T>(cmd, args);

describe("dev-mock M1 surface", () => {
  it("assembles a notebook tree with nested folders", async () => {
    const tree = await call<NotebookNode[]>("notebooks_list");
    const work = tree.find((n) => n.name === "Work");
    expect(work).toBeDefined();
    expect(work?.children.some((c) => c.name === "Research")).toBe(true);
  });

  it("daily.get_or_create is idempotent per date", async () => {
    const a = await call<Note>("daily_get_or_create", { date: "2026-07-23" });
    const b = await call<Note>("daily_get_or_create", { date: "2026-07-23" });
    expect(a.id).toBe(b.id);
  });

  it("projects wikilink backlinks between notes", async () => {
    const notes = await call<NoteSummary[]>("notes_list", { notebook_id: null });
    const reading = notes.find((n) => (n.title ?? "").startsWith("Reading"));
    expect(reading).toBeDefined();
    const backlinks = await call<BacklinkRef[]>("links_backlinks", { entity_id: reading!.id });
    expect(backlinks.some((b) => (b.source_title ?? "").startsWith("Product review"))).toBe(true);
  });

  it("creates a note from imported Markdown without overwriting", async () => {
    const before = (await call<NoteSummary[]>("notes_list", { notebook_id: null })).length;
    const note = await call<Note>("notes_import_markdown", { md: "# Imported\n\nBody line", notebook_id: null });
    expect(note.title).toBe("Imported");
    const after = (await call<NoteSummary[]>("notes_list", { notebook_id: null })).length;
    expect(after).toBe(before + 1);
  });
});
