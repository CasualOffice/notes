/**
 * Quick capture (HLD §8.2). One line of text goes to `capture.quick`; the core
 * routes it through `app-nlp` to a note, task, or reminder and returns where it
 * landed. We surface that routing so the user sees what happened, then let the
 * caller react (e.g. refresh the affected list).
 */
import { useState } from "react";
import { api, type CaptureResult } from "../../lib/api";

interface Props {
  onCaptured: (result: CaptureResult) => void;
}

const ROUTE_COPY: Record<string, string> = {
  note: "Filed as a note",
  task: "Added to tasks",
  reminder: "Scheduled a reminder",
};

export function QuickCapture({ onCaptured }: Props): React.JSX.Element {
  const [text, setText] = useState<string>("");
  const [hint, setHint] = useState<string>("");
  const [busy, setBusy] = useState<boolean>(false);

  const submit = async (): Promise<void> => {
    const value = text.trim();
    if (!value || busy) return;
    setBusy(true);
    try {
      const result = await api.captureQuick(value);
      const kind = result.entity_ref.kind;
      const copy = ROUTE_COPY[kind] ?? `Routed to ${kind}`;
      setHint(`${copy}: “${result.parsed.title}”`);
      setText("");
      onCaptured(result);
    } catch (e: unknown) {
      setHint(`Capture failed: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="capture">
      <div className="capture-row">
        <input
          value={text}
          placeholder="Capture anything — “call Sam tomorrow 3pm #work !2”"
          aria-label="Quick capture"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void submit();
          }}
        />
        <button type="button" className="btn btn-accent" onClick={() => void submit()} disabled={busy}>
          Capture
        </button>
      </div>
      <div className="capture-hint" aria-live="polite">
        {hint ? <span className="route">{hint}</span> : "Routed by intent to a note, task, or reminder."}
      </div>
    </div>
  );
}
