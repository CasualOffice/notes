/**
 * Backlinks panel (Feature Specs §3). For the open note it lists resolved inbound
 * links (`links.backlinks`) and prose mentions that are not yet links
 * (`links.unlinked_mentions`). Both are pure reads over the projection; clicking a
 * source opens it. Refreshes when the core signals a projection/backlink change.
 */
import { useCallback, useEffect, useState } from "react";
import { api, onAppEvent, type BacklinkRef, type UnlinkedMention } from "../../lib/api";

const REFRESH_EVENTS = new Set(["BacklinksChanged", "NoteProjected", "NoteSaved"]);

interface Props {
  noteId: string;
  onOpen: (noteId: string) => void;
}

export function Backlinks({ noteId, onOpen }: Props): React.JSX.Element {
  const [backlinks, setBacklinks] = useState<BacklinkRef[]>([]);
  const [mentions, setMentions] = useState<UnlinkedMention[]>([]);

  const refresh = useCallback(async (): Promise<void> => {
    const [b, m] = await Promise.all([
      api.linksBacklinks(noteId),
      api.linksUnlinkedMentions(noteId),
    ]);
    setBacklinks(b);
    setMentions(m);
  }, [noteId]);

  useEffect(() => {
    void refresh().catch(() => undefined);
  }, [refresh]);

  useEffect(() => {
    const unlisten = onAppEvent((ev) => {
      if (REFRESH_EVENTS.has(ev.type)) void refresh().catch(() => undefined);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refresh]);

  const total = backlinks.length + mentions.length;

  return (
    <section className="backlinks" aria-label="Backlinks">
      <header className="backlinks-head">
        <span className="backlinks-title">Linked references</span>
        <span className="backlinks-count">{total}</span>
      </header>

      {backlinks.length === 0 && mentions.length === 0 ? (
        <p className="backlinks-empty">
          No backlinks yet. Reference this note with <code>[[wikilinks]]</code> from elsewhere.
        </p>
      ) : (
        <>
          {backlinks.map((b) => (
            <button
              key={`b-${b.source_note_id}-${b.block_id ?? ""}`}
              type="button"
              className="backlink-item"
              onClick={() => onOpen(b.source_note_id)}
            >
              <span className="backlink-source">{b.source_title ?? "Untitled"}</span>
              {b.snippet && <span className="backlink-snippet">{b.snippet}</span>}
            </button>
          ))}

          {mentions.length > 0 && (
            <>
              <div className="backlinks-subhead">Unlinked mentions</div>
              {mentions.map((m) => (
                <button
                  key={`m-${m.source_note_id}`}
                  type="button"
                  className="backlink-item unlinked"
                  onClick={() => onOpen(m.source_note_id)}
                >
                  <span className="backlink-source">{m.source_title ?? "Untitled"}</span>
                  {m.snippet && <span className="backlink-snippet">{m.snippet}</span>}
                </button>
              ))}
            </>
          )}
        </>
      )}
    </section>
  );
}
