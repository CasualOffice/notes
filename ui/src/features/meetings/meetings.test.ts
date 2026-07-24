import { describe, expect, it } from "vitest";
import { mockCore } from "../../lib/mock";
import type {
  ActionItemViewT,
  AppEventEnvelope,
  MeetingArtifactV1,
  PreflightReportT,
  TranscriptSegmentT,
} from "../../lib/api";

const call = <T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> =>
  mockCore.invoke<T>(cmd, args);

/** Poll a growing event log until `pred` holds, or reject after `timeoutMs`. */
function waitFor(events: AppEventEnvelope[], pred: (e: AppEventEnvelope[]) => boolean, timeoutMs = 8000): Promise<void> {
  return new Promise((resolve, reject) => {
    const started = Date.now();
    const tick = (): void => {
      if (pred(events)) return resolve();
      if (Date.now() - started > timeoutMs) return reject(new Error("waitFor timed out"));
      setTimeout(tick, 20);
    };
    tick();
  });
}

describe("dev-mock meeting pipeline (M2)", () => {
  it("drives a session start → live → review with resolvable evidence", async () => {
    const events: AppEventEnvelope[] = [];
    const unlisten = mockCore.subscribe((e) => events.push(e));

    const preflight = await call<PreflightReportT>("meeting_preflight", { sources: [] });
    expect(preflight.ready).toBe(true);

    const sessionId = await call<string>("meeting_start", {
      sources: ["us.zoom.xos"],
      capture_microphone: true,
      title: "Test meeting",
    });
    expect(typeof sessionId).toBe("string");

    // Reaches RECORDING and streams provisional (live) transcript + a level meter.
    await waitFor(events, (es) =>
      es.some((e) => e.type === "SessionStateChanged" && e["to"] === "RECORDING"),
    );
    await waitFor(events, (es) =>
      es.some((e) => e.type === "LiveTranscript" && (e["segment"] as TranscriptSegmentT).pass === "live"),
    );
    expect(events.some((e) => e.type === "CaptureLevel")).toBe(true);

    // Stop drives the tail to COMPLETE (capture already done — never rewinds).
    await call<string>("meeting_stop", { session_id: sessionId });
    await waitFor(events, (es) =>
      es.some((e) => e.type === "SessionStateChanged" && e["to"] === "COMPLETE"),
    );
    expect(events.some((e) => e.type === "ArtifactReady")).toBe(true);

    const transcript = await call<TranscriptSegmentT[]>("meeting_transcript", { session_id: sessionId });
    const artifact = await call<MeetingArtifactV1>("meeting_artifact", { session_id: sessionId });
    const ids = new Set(transcript.map((s) => s.segment_id));

    // Evidence or nothing: every fact cites resolvable segments.
    expect(artifact.topics.length).toBeGreaterThan(0);
    for (const topic of artifact.topics) {
      expect(topic.evidence_segment_ids.length).toBeGreaterThan(0);
      expect(topic.evidence_segment_ids.every((id) => ids.has(id))).toBe(true);
    }
    for (const item of artifact.action_items) {
      expect(item.evidence_segment_ids.every((id) => ids.has(id))).toBe(true);
    }
    unlisten();
  }, 12000);

  it("promotes a suggested action item to a task", async () => {
    const sessionId = await call<string>("meeting_start", { sources: [], capture_microphone: true, title: null });
    await call<string>("meeting_stop", { session_id: sessionId });
    // Poll the action-items endpoint until the pipeline has filed them.
    let items = await call<ActionItemViewT[]>("meeting_action_items", { session_id: sessionId });
    expect(items.length).toBeGreaterThan(0);

    const first = items[0]!;
    const taskId = await call<string>("meeting_action_item_to_task", {
      session_id: sessionId,
      action_item_id: first.id,
    });
    expect(typeof taskId).toBe("string");

    items = await call<ActionItemViewT[]>("meeting_action_items", { session_id: sessionId });
    expect(items[0]!.status).toBe("promoted");
    expect(items[0]!.promoted_task_id).toBe(taskId);
  });
});
