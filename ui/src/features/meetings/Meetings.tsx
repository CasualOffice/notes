/**
 * The Meetings experience (HLD §8.4) — the fourth pillar. Routes a single session
 * through the source picker → live recording → processing → evidence-cited review,
 * driven entirely by `meeting.*` commands and the `AppEvent` stream. The WebView
 * never owns recording state; it reacts to what the core reports.
 */
import { SourcePicker } from "./SourcePicker";
import { LiveView } from "./LiveView";
import { ReviewView } from "./ReviewView";
import { useMeetingSession } from "./useMeetingSession";

export function Meetings(): React.JSX.Element {
  const session = useMeetingSession();
  const { phase } = session;

  return (
    <div className="mtg">
      {(phase === "live" || phase === "processing" || phase === "review" || phase === "failed") && (
        <div className="mtg-toolbar">
          <span className="mtg-toolbar-title">
            {phase === "review" ? "Meeting review" : phase === "processing" ? "Processing" : phase === "failed" ? "Meeting" : "Recording"}
          </span>
          {(phase === "review" || phase === "failed") && (
            <button type="button" className="btn btn-ghost" onClick={session.reset}>
              New meeting
            </button>
          )}
        </div>
      )}

      {session.error && (
        <div className="error-banner" role="alert">
          {session.error}
        </div>
      )}

      <div className="mtg-stage">
        {phase === "idle" && <SourcePicker onStart={(cfg) => void session.start(cfg)} />}
        {(phase === "live" || phase === "processing") && <LiveView session={session} />}
        {(phase === "review" || phase === "failed") && <ReviewView session={session} />}
      </div>
    </div>
  );
}
