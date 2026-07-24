/**
 * Meeting source picker (HLD §9.1). Lists the capturable applications, lets the
 * user toggle app sources + the microphone, runs `meeting.preflight` to gate the
 * arm affordance on an honest capability/permission report, then `meeting.start`.
 *
 * Capability honesty: the arm button stays disabled until preflight reports
 * `ready`, and platform limits (best-effort app audio, running-only, portal
 * consent) are surfaced rather than hidden.
 */
import { useEffect, useState } from "react";
import { api, type CapturableAppT, type MeetingStartConfig, type PreflightReportT } from "../../lib/api";

interface Props {
  onStart: (config: MeetingStartConfig) => void;
}

const SUPPORT_COPY: Record<string, string> = {
  supported: "First-class app audio capture",
  best_effort: "App audio capture is best-effort on this platform",
  unsupported: "App-level audio capture is not available here",
};

export function SourcePicker({ onStart }: Props): React.JSX.Element {
  const [apps, setApps] = useState<CapturableAppT[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [captureMic, setCaptureMic] = useState<boolean>(true);
  const [title, setTitle] = useState<string>("");
  const [preflight, setPreflight] = useState<PreflightReportT | null>(null);
  const [error, setError] = useState<string>("");

  useEffect(() => {
    void api
      .meetingListApps()
      .then(setApps)
      .catch((e: unknown) => setError(String(e)));
  }, []);

  // Re-run preflight whenever the chosen sources change (honest capability gate).
  useEffect(() => {
    void api
      .meetingPreflight([...selected])
      .then(setPreflight)
      .catch((e: unknown) => setError(String(e)));
  }, [selected]);

  const toggle = (appId: string): void => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(appId)) next.delete(appId);
      else next.add(appId);
      return next;
    });
  };

  const canArm = (preflight?.ready ?? false) && (selected.size > 0 || captureMic);

  const arm = (): void => {
    onStart({
      sources: [...selected],
      captureMicrophone: captureMic,
      title: title.trim() || null,
    });
  };

  const support = preflight?.capabilities.app_level_audio;

  return (
    <div className="mtg-picker">
      <div className="mtg-picker-head">
        <h1 className="mtg-title">New meeting</h1>
        <p className="mtg-sub">
          Choose which applications to capture. Audio stays on this device — nothing is
          uploaded.
        </p>
      </div>

      {error && (
        <div className="error-banner" role="alert">
          {error}
        </div>
      )}

      <label className="mtg-field">
        <span className="mtg-field-label">Meeting title</span>
        <input
          className="mtg-input"
          value={title}
          placeholder="Q3 roadmap review"
          onChange={(e) => setTitle(e.target.value)}
        />
      </label>

      <div className="mtg-section-label">Capture sources</div>
      <ul className="mtg-app-list">
        {apps.map((app) => {
          const on = selected.has(app.app_id);
          return (
            <li key={app.app_id}>
              <button
                type="button"
                className={`mtg-app${on ? " on" : ""}`}
                aria-pressed={on}
                onClick={() => toggle(app.app_id)}
              >
                <span className="mtg-app-check" aria-hidden="true">
                  {on ? "✓" : ""}
                </span>
                <span className="mtg-app-body">
                  <span className="mtg-app-name">{app.display_name}</span>
                  <span className="mtg-app-meta">
                    {app.produces_audio ? "Producing audio now" : "Silent"}
                  </span>
                </span>
              </button>
            </li>
          );
        })}
        {apps.length === 0 && <li className="mtg-empty">No capturable applications found.</li>}
      </ul>

      <button
        type="button"
        className={`mtg-mic${captureMic ? " on" : ""}`}
        aria-pressed={captureMic}
        onClick={() => setCaptureMic((v) => !v)}
      >
        <span className="mtg-app-check" aria-hidden="true">
          {captureMic ? "✓" : ""}
        </span>
        <span className="mtg-app-body">
          <span className="mtg-app-name">Microphone</span>
          <span className="mtg-app-meta">Capture your voice as a separate track</span>
        </span>
      </button>

      {support && support !== "supported" && (
        <p className="mtg-capability" role="note">
          {SUPPORT_COPY[support]}
        </p>
      )}
      {preflight && !preflight.ready && (
        <p className="mtg-capability warn" role="note">
          Capture is not ready — grant capture permission in system settings.
        </p>
      )}

      <div className="mtg-picker-actions">
        <button type="button" className="btn btn-accent mtg-arm" disabled={!canArm} onClick={arm}>
          Start recording
        </button>
        <span className="mtg-arm-hint">
          {canArm ? "Ready to record" : "Select a source or the microphone to begin"}
        </span>
      </div>
    </div>
  );
}
