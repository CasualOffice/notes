/**
 * Meeting review (HLD §8.4, Data Model §14.1). Tabbed reading of the evidence-
 * resolved `MeetingArtifactV1`: Transcript / Summary / Decisions / Actions.
 *
 * Evidence or nothing: every displayed fact carries resolvable
 * `evidence_segment_ids`. Clicking a fact jumps to the Transcript tab and highlights
 * the exact segments that ground it. Action items promote to real Tasks via
 * `meeting.actionItemToTask` (which writes the `spawned_from` link + evidence).
 */
import { useEffect, useRef, useState } from "react";
import type { ActionItemViewT, TranscriptSegmentT } from "../../lib/api";
import type { MeetingSession } from "./useMeetingSession";

type Tab = "transcript" | "summary" | "decisions" | "actions";

const TABS: { id: Tab; label: string }[] = [
  { id: "transcript", label: "Transcript" },
  { id: "summary", label: "Summary" },
  { id: "decisions", label: "Decisions" },
  { id: "actions", label: "Actions" },
];

interface Props {
  session: MeetingSession;
}

function tsLabel(ms: number): string {
  const total = Math.floor(ms / 1000);
  const mm = String(Math.floor(total / 60)).padStart(2, "0");
  const ss = String(total % 60).padStart(2, "0");
  return `${mm}:${ss}`;
}

export function ReviewView({ session }: Props): React.JSX.Element {
  const { artifact, transcript, actionItems, degradedReason } = session;
  const [tab, setTab] = useState<Tab>("summary");
  const [highlighted, setHighlighted] = useState<string[]>([]);
  const segRefs = useRef<Map<string, HTMLLIElement>>(new Map());

  // Jump to the Transcript tab + scroll the first cited segment into view.
  const cite = (ids: string[]): void => {
    setHighlighted(ids);
    setTab("transcript");
  };

  useEffect(() => {
    if (tab !== "transcript" || highlighted.length === 0) return;
    const first = highlighted[0];
    if (!first) return;
    const el = segRefs.current.get(first);
    el?.scrollIntoView({ behavior: "smooth", block: "center" });
  }, [tab, highlighted]);

  const evidenceButton = (ids: string[]): React.JSX.Element => (
    <button type="button" className="mtg-evidence" onClick={() => cite(ids)} title="Show supporting transcript">
      {ids.length} {ids.length === 1 ? "citation" : "citations"}
    </button>
  );

  return (
    <div className="mtg-review">
      <div className="mtg-tabs" role="tablist">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            className={`mtg-tab${tab === t.id ? " active" : ""}`}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>

      {degradedReason && (
        <div className="mtg-degraded" role="note">
          Summary unavailable ({degradedReason}). The transcript is preserved below.
        </div>
      )}

      <div className="mtg-review-body">
        {tab === "transcript" && (
          <TranscriptTab transcript={transcript} highlighted={new Set(highlighted)} segRefs={segRefs} />
        )}

        {tab === "summary" && artifact && (
          <div className="mtg-panel">
            <p className="mtg-exec">{artifact.executive_summary || "No summary was produced."}</p>
            {artifact.topics.map((topic, i) => (
              <div key={i} className="mtg-fact">
                <div className="mtg-fact-main">
                  <span className="mtg-fact-title">{topic.title}</span>
                  <span className="mtg-fact-text">{topic.summary}</span>
                </div>
                {evidenceButton(topic.evidence_segment_ids)}
              </div>
            ))}
            {artifact.risks.length > 0 && <div className="mtg-subhead">Risks</div>}
            {artifact.risks.map((risk, i) => (
              <div key={i} className="mtg-fact">
                <div className="mtg-fact-main">
                  <span className="mtg-fact-text">{risk.statement}</span>
                </div>
                {evidenceButton(risk.evidence_segment_ids)}
              </div>
            ))}
            {artifact.open_questions.length > 0 && <div className="mtg-subhead">Open questions</div>}
            {artifact.open_questions.map((q, i) => (
              <div key={i} className="mtg-fact">
                <div className="mtg-fact-main">
                  <span className="mtg-fact-text">{q.question}</span>
                </div>
                {evidenceButton(q.evidence_segment_ids)}
              </div>
            ))}
          </div>
        )}

        {tab === "decisions" && artifact && (
          <div className="mtg-panel">
            {artifact.decisions.length === 0 && <p className="mtg-transcript-empty">No decisions recorded.</p>}
            {artifact.decisions.map((d, i) => (
              <div key={i} className="mtg-fact">
                <div className="mtg-fact-main">
                  <span className="mtg-fact-text">{d.statement}</span>
                  {d.rationale && <span className="mtg-fact-note">Why: {d.rationale}</span>}
                </div>
                {evidenceButton(d.evidence_segment_ids)}
              </div>
            ))}
          </div>
        )}

        {tab === "actions" && (
          <div className="mtg-panel">
            {actionItems.length === 0 && <p className="mtg-transcript-empty">No action items were extracted.</p>}
            {actionItems.map((item) => (
              <ActionRow key={item.id} item={item} onCite={cite} onAdd={session.addToTasks} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

interface TranscriptTabProps {
  transcript: TranscriptSegmentT[];
  highlighted: Set<string>;
  segRefs: React.MutableRefObject<Map<string, HTMLLIElement>>;
}

function TranscriptTab({ transcript, highlighted, segRefs }: TranscriptTabProps): React.JSX.Element {
  if (transcript.length === 0) {
    return <p className="mtg-transcript-empty">The transcript is empty.</p>;
  }
  return (
    <ul className="mtg-transcript full">
      {transcript.map((seg) => {
        const on = highlighted.has(seg.segment_id);
        return (
          <li
            key={seg.segment_id}
            ref={(el) => {
              if (el) segRefs.current.set(seg.segment_id, el);
              else segRefs.current.delete(seg.segment_id);
            }}
            className={`mtg-seg final${on ? " cited" : ""}`}
          >
            <span className="mtg-seg-time">{tsLabel(seg.t_start_ms)}</span>
            <div className="mtg-seg-body">
              {seg.speaker && <span className="mtg-seg-speaker">{seg.speaker}</span>}
              <span className="mtg-seg-text">{seg.text}</span>
            </div>
          </li>
        );
      })}
    </ul>
  );
}

interface ActionRowProps {
  item: ActionItemViewT;
  onCite: (ids: string[]) => void;
  onAdd: (id: string) => Promise<void>;
}

function ActionRow({ item, onCite, onAdd }: ActionRowProps): React.JSX.Element {
  const [busy, setBusy] = useState<boolean>(false);
  const promoted = item.status === "promoted" || item.promoted_task_id !== null;

  const add = async (): Promise<void> => {
    if (busy || promoted) return;
    setBusy(true);
    try {
      await onAdd(item.id);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="mtg-action">
      <div className="mtg-fact-main">
        <span className="mtg-fact-text">{item.task_text}</span>
        <span className="mtg-action-meta">
          {item.owner_text && <span className="mtg-chip">{item.owner_text}</span>}
          {item.due_date && <span className="mtg-chip">Due {item.due_date}</span>}
          <button
            type="button"
            className="mtg-evidence"
            onClick={() => onCite(item.evidence_segment_ids)}
            title="Show supporting transcript"
          >
            {item.evidence_segment_ids.length} citation
            {item.evidence_segment_ids.length === 1 ? "" : "s"}
          </button>
        </span>
      </div>
      <button
        type="button"
        className={`btn${promoted ? "" : " btn-accent"} mtg-add-task`}
        disabled={promoted || busy}
        onClick={() => void add()}
      >
        {promoted ? "Added to tasks" : busy ? "Adding…" : "Add to tasks"}
      </button>
    </div>
  );
}
