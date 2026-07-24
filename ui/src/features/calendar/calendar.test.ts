import { describe, expect, it } from "vitest";
import { mockCore } from "../../lib/mock";
import type { AgendaEvent } from "../../lib/api";

const call = <T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> =>
  mockCore.invoke<T>(cmd, args);

const DAY = 86_400_000;

describe("dev-mock calendar agenda projection", () => {
  it("merges tasks/reminders/meetings, clips to the window, and sorts by start", async () => {
    const from = Date.now() - 3 * DAY;
    const to = Date.now() + 8 * DAY;
    const evs = await call<AgendaEvent[]>("calendar_agenda", { from_ms: from, to_ms: to });

    expect(evs.length).toBeGreaterThan(0);

    // Sorted ascending by start.
    for (let i = 1; i < evs.length; i += 1) {
      expect(evs[i]!.start_ms).toBeGreaterThanOrEqual(evs[i - 1]!.start_ms);
    }

    // Every event overlaps the requested window.
    for (const e of evs) {
      expect(e.start_ms).toBeLessThanOrEqual(to);
      expect(e.end_ms).toBeGreaterThanOrEqual(from);
    }

    // The three source pillars are all represented.
    const sources = new Set(evs.map((e) => e.source));
    expect(sources.has("meeting")).toBe(true);
    expect(sources.has("reminder")).toBe(true);
    expect(sources.has("task")).toBe(true);
  });

  it("exports a well-formed RFC 5545 ICS document", async () => {
    const from = Date.now() - 3 * DAY;
    const to = Date.now() + 8 * DAY;
    const ics = await call<string>("calendar_export_ics", { from_ms: from, to_ms: to });

    expect(ics.startsWith("BEGIN:VCALENDAR")).toBe(true);
    expect(ics).toContain("END:VCALENDAR");
    expect(ics).toContain("BEGIN:VEVENT");
    expect(ics).toContain("SUMMARY:");
  });
});
