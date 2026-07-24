/**
 * Live recording view (HLD §8.4). A recording indicator, an audio-level meter fed
 * by throttled `CaptureLevel` events, the streaming pass-1 transcript (provisional
 * rows styled distinctly from finals), an elapsed timer, and pause/resume/stop.
 *
 * While the tail of the pipeline runs (STOPPING → … → INDEXING) the same surface
 * shows honest processing status — capture is already done, so this never blocks.
 */
import type { MeetingSession } from "./useMeetingSession";

interface Props {
  session: MeetingSession;
}

/** Map an RMS level in dBFS (roughly −60..0) to a 0..1 meter fill. */
function meterFill(dbfs: number): number {
  const clamped = Math.max(-55, Math.min(-3, dbfs));
  return (clamped + 55) / 52;
}

function formatElapsed(ms: number): string {
  const total = Math.floor(ms / 1000);
  const mm = String(Math.floor(total / 60)).padStart(2, "0");
  const ss = String(total % 60).padStart(2, "0");
  return `${mm}:${ss}`;
}

const PROCESSING_COPY: Record<string, string> = {
  STOPPING: "Finishing capture…",
  CAPTURED: "Capture complete",
  FINAL_TRANSCRIBING: "Transcribing (final pass)…",
  GENERATING: "Summarizing with evidence…",
  INDEXING: "Filing into your notebook…",
};

export function LiveView({ session }: Props): React.JSX.Element {
  const { state, level, elapsedMs, liveSegments, indexing } = session;
  const recording = state === "RECORDING";
  const paused = state === "PAUSED";
  const live = recording || paused;
  const fill = meterFill(level);

  return (
    <div className="mtg-live">
      <div className="mtg-live-bar">
        <div className="mtg-live-status">
          <span className={`mtg-rec-dot${recording ? " on" : paused ? " paused" : ""}`} aria-hidden="true" />
          <span className="mtg-rec-label">
            {recording ? "Recording" : paused ? "Paused" : (PROCESSING_COPY[state ?? ""] ?? "Working…")}
          </span>
          <span className="mtg-elapsed" aria-label="Elapsed time">
            {formatElapsed(elapsedMs)}
          </span>
        </div>

        <div className="mtg-meter" aria-hidden={!live}>
          <div className="mtg-meter-track">
            <div className="mtg-meter-fill" style={{ width: `${Math.round(fill * 100)}%` }} />
          </div>
          <span className="mtg-meter-db">{live ? `${Math.round(level)} dBFS` : ""}</span>
        </div>

        <div className="mtg-live-controls">
          {live && (
            <>
              {recording ? (
                <button type="button" className="btn" onClick={session.pause}>
                  Pause
                </button>
              ) : (
                <button type="button" className="btn" onClick={session.resume}>
                  Resume
                </button>
              )}
              <button type="button" className="btn btn-accent" onClick={session.stop}>
                Stop
              </button>
            </>
          )}
        </div>
      </div>

      {!live && indexing && (
        <div className="mtg-progress" role="status">
          <div className="mtg-progress-track">
            <div className="mtg-progress-fill" style={{ width: `${Math.round(indexing.pct * 100)}%` }} />
          </div>
          <span className="mtg-progress-label">{PROCESSING_COPY[state ?? ""] ?? indexing.stage}</span>
        </div>
      )}

      <div className="mtg-transcript-scroll">
        {liveSegments.length === 0 ? (
          <p className="mtg-transcript-empty">
            {recording ? "Listening… the live transcript will appear here." : "Preparing capture…"}
          </p>
        ) : (
          <ul className="mtg-transcript">
            {liveSegments.map((seg, i) => (
              <li
                key={`${seg.segment_id}-${i}`}
                className={`mtg-seg${seg.pass === "live" ? " provisional" : " final"}`}
              >
                {seg.speaker && <span className="mtg-seg-speaker">{seg.speaker}</span>}
                <span className="mtg-seg-text">{seg.text}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
