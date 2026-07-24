/**
 * The Meetings state machine, WebView side (HLD §8.4 + §7). Drives one session
 * through the source picker → live recording → processing → review, reconciling
 * everything from the single `AppEvent` channel (`SessionStateChanged`,
 * `CaptureLevel`, `LiveTranscript`, `ArtifactReady`, `IndexingProgress`).
 *
 * The WebView never owns recording state: it issues `meeting.*` commands and reacts
 * to the events the core pushes back. A slow generation stage cannot rewind
 * recording — capture has already completed by the time GENERATING runs.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  onAppEvent,
  type ActionItemViewT,
  type AppEventEnvelope,
  type MeetingArtifactV1,
  type MeetingStartConfig,
  type TranscriptSegmentT,
} from "../../lib/api";

/** The coarse UI phase derived from the fine-grained `SessionState`. */
export type MeetingPhase = "idle" | "live" | "processing" | "review" | "failed";

/** Which STT pass a `SessionState` maps onto for the phase machine. */
const LIVE_STATES = new Set(["NEW", "PREFLIGHT", "READY", "RECORDING", "PAUSED"]);
const PROCESSING_STATES = new Set([
  "STOPPING",
  "CAPTURED",
  "FINAL_TRANSCRIBING",
  "GENERATING",
  "INDEXING",
]);

function phaseOf(state: string | null): MeetingPhase {
  if (state === null) return "idle";
  if (LIVE_STATES.has(state)) return "live";
  if (PROCESSING_STATES.has(state)) return "processing";
  if (state === "COMPLETE") return "review";
  return "failed"; // DEGRADED / FAILED
}

export interface IndexingProgress {
  stage: string;
  pct: number;
}

export interface MeetingSession {
  phase: MeetingPhase;
  state: string | null;
  sessionId: string | null;
  /** Streamed transcript rows — provisional (`live`) and, later, `final`. */
  liveSegments: TranscriptSegmentT[];
  /** Latest capture RMS level in dBFS (throttled meter). */
  level: number;
  elapsedMs: number;
  indexing: IndexingProgress | null;
  degradedReason: string | null;
  artifact: MeetingArtifactV1 | null;
  transcript: TranscriptSegmentT[];
  actionItems: ActionItemViewT[];
  error: string;
  start: (config: MeetingStartConfig) => Promise<void>;
  pause: () => void;
  resume: () => void;
  stop: () => void;
  addToTasks: (actionItemId: string) => Promise<void>;
  reset: () => void;
}

export function useMeetingSession(): MeetingSession {
  const [state, setState] = useState<string | null>(null);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [liveSegments, setLiveSegments] = useState<TranscriptSegmentT[]>([]);
  const [level, setLevel] = useState<number>(-60);
  const [elapsedMs, setElapsedMs] = useState<number>(0);
  const [indexing, setIndexing] = useState<IndexingProgress | null>(null);
  const [degradedReason, setDegradedReason] = useState<string | null>(null);
  const [artifact, setArtifact] = useState<MeetingArtifactV1 | null>(null);
  const [transcript, setTranscript] = useState<TranscriptSegmentT[]>([]);
  const [actionItems, setActionItems] = useState<ActionItemViewT[]>([]);
  const [error, setError] = useState<string>("");

  const sessionRef = useRef<string | null>(null);

  const loadReview = useCallback(async (id: string): Promise<void> => {
    try {
      const [art, tx, items] = await Promise.all([
        api.meetingArtifact(id),
        api.meetingTranscript(id),
        api.meetingActionItems(id),
      ]);
      setArtifact(art);
      setTranscript(tx);
      setActionItems(items);
    } catch (e: unknown) {
      setError(String(e));
    }
  }, []);

  // Single subscription to the core→WebView channel (HLD §7). Handlers use
  // functional setters + a session ref so the effect never needs to re-subscribe.
  useEffect(() => {
    const unlisten = onAppEvent((ev: AppEventEnvelope) => {
      if (ev["session_id"] !== undefined && ev["session_id"] !== sessionRef.current) return;
      switch (ev.type) {
        case "SessionStateChanged": {
          const to = String(ev["to"]);
          setState(to);
          const degraded = ev["degraded"];
          setDegradedReason(typeof degraded === "string" ? degraded : null);
          if (to === "COMPLETE" && sessionRef.current) void loadReview(sessionRef.current);
          break;
        }
        case "CaptureLevel":
          setLevel(Number(ev["rms_dbfs"] ?? -60));
          break;
        case "LiveTranscript": {
          const segment = ev["segment"] as TranscriptSegmentT | undefined;
          if (segment) setLiveSegments((prev) => [...prev, segment]);
          break;
        }
        case "IndexingProgress":
          setIndexing({ stage: String(ev["stage"]), pct: Number(ev["pct"] ?? 0) });
          break;
        default:
          break;
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [loadReview]);

  // Elapsed timer — advances only while RECORDING (freezes on pause).
  useEffect(() => {
    if (state !== "RECORDING") return;
    const t = setInterval(() => setElapsedMs((ms) => ms + 1000), 1000);
    return () => clearInterval(t);
  }, [state]);

  const reset = useCallback((): void => {
    sessionRef.current = null;
    setSessionId(null);
    setState(null);
    setLiveSegments([]);
    setLevel(-60);
    setElapsedMs(0);
    setIndexing(null);
    setDegradedReason(null);
    setArtifact(null);
    setTranscript([]);
    setActionItems([]);
    setError("");
  }, []);

  const start = useCallback(
    async (config: MeetingStartConfig): Promise<void> => {
      reset();
      try {
        const id = await api.meetingStart(config);
        sessionRef.current = id;
        setSessionId(id);
      } catch (e: unknown) {
        setError(String(e));
      }
    },
    [reset],
  );

  const pause = useCallback((): void => {
    if (sessionRef.current) void api.meetingPause(sessionRef.current).catch((e: unknown) => setError(String(e)));
  }, []);

  const resume = useCallback((): void => {
    if (sessionRef.current) void api.meetingResume(sessionRef.current).catch((e: unknown) => setError(String(e)));
  }, []);

  const stop = useCallback((): void => {
    if (sessionRef.current) void api.meetingStop(sessionRef.current).catch((e: unknown) => setError(String(e)));
  }, []);

  const addToTasks = useCallback(async (actionItemId: string): Promise<void> => {
    const id = sessionRef.current;
    if (!id) return;
    try {
      await api.meetingActionItemToTask(id, actionItemId);
      setActionItems(await api.meetingActionItems(id));
    } catch (e: unknown) {
      setError(String(e));
    }
  }, []);

  return {
    phase: phaseOf(state),
    state,
    sessionId,
    liveSegments,
    level,
    elapsedMs,
    indexing,
    degradedReason,
    artifact,
    transcript,
    actionItems,
    error,
    start,
    pause,
    resume,
    stop,
    addToTasks,
    reset,
  };
}
