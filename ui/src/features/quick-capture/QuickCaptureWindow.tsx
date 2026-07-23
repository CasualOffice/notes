/**
 * The frameless global quick-capture window (HLD §8.2, window label
 * "quick-capture"). One line of text goes to `capture.quick`, which routes it via
 * `app-nlp` to a note, task, or reminder; the routed result is shown briefly and
 * the window hides. Escape hides immediately. The window is pre-created hidden and
 * toggled by the tray / global hotkey, so we hide (not close) to keep it reusable.
 */
import { useEffect, useRef, useState } from "react";
import { api, hideCurrentWindow, type CaptureResult } from "../../lib/api";

const ROUTE_COPY: Record<string, string> = {
  note: "Filed as a note",
  task: "Added to tasks",
  reminder: "Scheduled a reminder",
};

export function QuickCaptureWindow(): React.JSX.Element {
  const [text, setText] = useState<string>("");
  const [result, setResult] = useState<CaptureResult | null>(null);
  const [busy, setBusy] = useState<boolean>(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const dismiss = (): void => {
    setText("");
    setResult(null);
    void hideCurrentWindow();
  };

  const submit = async (): Promise<void> => {
    const value = text.trim();
    if (!value || busy) return;
    setBusy(true);
    try {
      const r = await api.captureQuick(value);
      setResult(r);
      setText("");
      // Show the routing outcome for a beat, then hide the window.
      window.setTimeout(dismiss, 900);
    } catch (e: unknown) {
      setResult(null);
      console.error("quick capture failed", e);
    } finally {
      setBusy(false);
    }
  };

  const routed = result
    ? `${ROUTE_COPY[result.entity_ref.kind] ?? `Routed to ${result.entity_ref.kind}`}: “${result.parsed.title}”`
    : "";

  return (
    <div className="qc-window">
      <input
        ref={inputRef}
        className="qc-input"
        value={text}
        placeholder="Capture anything — “call Sam tomorrow 3pm #work !2”"
        aria-label="Quick capture"
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") void submit();
          else if (e.key === "Escape") dismiss();
        }}
      />
      <div className="qc-hint" aria-live="polite">
        {routed ? (
          <span className="route">{routed}</span>
        ) : (
          "Enter to capture · Esc to close · routed by intent"
        )}
      </div>
    </div>
  );
}
