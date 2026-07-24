/**
 * The AI workspace (Feature Specs §7, HLD §8.5) — grounded question answering.
 * A query runs through `ai.ask`, which returns an `AnswerV1`: either a grounded
 * answer whose every citation resolves to a real note block / transcript window
 * (evidence or nothing), or an honest refusal with `unanswered: true`. Citations
 * into notes are click-through; the answer is never shown without its evidence.
 */
import { useCallback, useState } from "react";
import { api, type AnswerV1, type Citation } from "../../lib/api";

interface Props {
  /** Open a note-block citation's source note in the Notes pillar. */
  onOpenNote?: (noteId: string) => void;
}

function kindLabel(kind: Citation["source_kind"]): string {
  switch (kind) {
    case "note_block":
      return "Note";
    case "transcript_window":
      return "Meeting";
    case "task":
      return "Task";
    case "reminder":
      return "Reminder";
    default:
      return "Source";
  }
}

function tsLabel(ms: number): string {
  const total = Math.floor(ms / 1000);
  const mm = String(Math.floor(total / 60)).padStart(2, "0");
  const ss = String(total % 60).padStart(2, "0");
  return `${mm}:${ss}`;
}

export function AskView({ onOpenNote }: Props): React.JSX.Element {
  const [query, setQuery] = useState<string>("");
  const [answer, setAnswer] = useState<AnswerV1 | null>(null);
  const [asked, setAsked] = useState<string>("");
  const [loading, setLoading] = useState<boolean>(false);
  const [error, setError] = useState<string>("");

  const ask = useCallback(async (): Promise<void> => {
    const q = query.trim();
    if (!q || loading) return;
    setLoading(true);
    setError("");
    setAnswer(null);
    setAsked(q);
    try {
      setAnswer(await api.aiAsk(q));
    } catch (e: unknown) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [query, loading]);

  const openCitation = (c: Citation): void => {
    if (c.source_kind === "note_block") onOpenNote?.(c.source_id);
  };

  return (
    <div className="ask">
      <div className="ask-inner">
        <header className="ask-head">
          <h1 className="ask-h1">Ask your notes</h1>
          <p className="ask-sub">
            Answers are grounded in your notes and meetings, with citations you can open. If the evidence isn't
            there, you'll get an honest "I don't know" — never a guess.
          </p>
        </header>

        <form
          className="ask-form"
          onSubmit={(e) => {
            e.preventDefault();
            void ask();
          }}
        >
          <input
            className="ask-input"
            placeholder="e.g. What did we decide about capture latency?"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            aria-label="Question"
          />
          <button type="submit" className="btn btn-accent" disabled={!query.trim() || loading}>
            {loading ? "Thinking…" : "Ask"}
          </button>
        </form>

        {error && (
          <div className="error-banner" role="alert">
            {error}
          </div>
        )}

        {loading && <p className="ask-status">Searching your notes for grounded evidence…</p>}

        {answer && !loading && (
          <div className="ask-answer">
            <div className="ask-question">{asked}</div>
            {answer.unanswered ? (
              <div className="ask-unknown">
                <span className="ask-unknown-title">I don't know</span>
                <p className="ask-unknown-body">
                  {answer.answer || "I couldn't find grounded evidence for that in your notes."}
                </p>
              </div>
            ) : (
              <>
                <p className="ask-answer-text">{answer.answer}</p>
                <div className="ask-answer-meta">
                  <span className="ask-confidence">Confidence {Math.round(answer.confidence * 100)}%</span>
                  <span className="ask-cite-count">
                    {answer.citations.length} {answer.citations.length === 1 ? "citation" : "citations"}
                  </span>
                </div>
                <div className="ask-subhead">Evidence</div>
                <ul className="ask-citations">
                  {answer.citations.map((c) => {
                    const clickable = c.source_kind === "note_block";
                    return (
                      <li key={c.chunk_id}>
                        <button
                          type="button"
                          className={`ask-citation${clickable ? " open" : ""}`}
                          onClick={() => openCitation(c)}
                          disabled={!clickable}
                          title={clickable ? "Open the source note" : "Supporting evidence"}
                        >
                          <span className="ask-citation-head">
                            <span className={`ask-citation-kind src-${c.source_kind}`}>
                              {kindLabel(c.source_kind)}
                            </span>
                            {c.t_start_ms != null && (
                              <span className="ask-citation-time">{tsLabel(c.t_start_ms)}</span>
                            )}
                          </span>
                          <span className="ask-citation-snippet">{c.snippet}</span>
                        </button>
                      </li>
                    );
                  })}
                </ul>
              </>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
