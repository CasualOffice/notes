import { describe, expect, it } from "vitest";
import { mockCore } from "../../lib/mock";
import type { AnswerV1, NoteView } from "../../lib/api";

const call = <T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> =>
  mockCore.invoke<T>(cmd, args);

describe("dev-mock ai.ask (evidence or nothing)", () => {
  it("returns a grounded answer whose note citation resolves", async () => {
    const a = await call<AnswerV1>("ai_ask", { query: "What did we decide about capture latency?" });

    expect(a.schema).toBe("AnswerV1");
    expect(a.unanswered).toBe(false);
    expect(a.citations.length).toBeGreaterThan(0);

    const noteCite = a.citations.find((c) => c.source_kind === "note_block");
    expect(noteCite).toBeDefined();

    // N14: every citation on a grounded answer resolves to a real source.
    const note = await call<NoteView>("notes_get", { note_id: noteCite!.source_id });
    expect(note.id).toBe(noteCite!.source_id);
  });

  it("refuses honestly, with no citations, when there is no evidence", async () => {
    const a = await call<AnswerV1>("ai_ask", { query: "What is the airspeed of a swallow?" });
    expect(a.unanswered).toBe(true);
    expect(a.citations).toHaveLength(0);
  });
});
