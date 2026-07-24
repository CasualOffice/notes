/**
 * The Calendar pillar (Feature Specs §3.4, §6) — a read-mostly *unified agenda*.
 * Tasks (with dates), reminders, and meetings are projected into one event stream
 * by the core via `calendar.agenda` (no separate event store in this pass). Two
 * readings of the same window: a grouped-by-day agenda and a month grid. Events are
 * tagged by their source with restrained, distinct styling; "Export .ics" emits an
 * RFC 5545 document for the visible month.
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { api, onAppEvent, type AgendaEvent } from "../../lib/api";

type Mode = "agenda" | "month";

interface Props {
  /** Jump from an event to its originating pillar (task/meeting), when possible. */
  onOpenSource?: (source: AgendaEvent["source"], id: string) => void;
}

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

const two = (n: number): string => String(n).padStart(2, "0");

function ymd(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}`;
}

function todayKey(): string {
  const d = new Date();
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}`;
}

function timeLabel(ms: number): string {
  const d = new Date(ms);
  const h = d.getHours();
  const m = d.getMinutes();
  const am = h < 12;
  const h12 = h % 12 === 0 ? 12 : h % 12;
  return `${h12}:${two(m)}${am ? "am" : "pm"}`;
}

function dayHeading(key: string): string {
  const [y, m, d] = key.split("-").map(Number);
  const date = new Date(y ?? 1970, (m ?? 1) - 1, d ?? 1);
  return `${WEEKDAYS[date.getDay()]}, ${MONTHS[date.getMonth()]?.slice(0, 3)} ${date.getDate()}`;
}

export function CalendarView({ onOpenSource }: Props): React.JSX.Element {
  // `anchor` is the first day of the visible month.
  const [anchor, setAnchor] = useState<Date>(() => {
    const d = new Date();
    return new Date(d.getFullYear(), d.getMonth(), 1);
  });
  const [mode, setMode] = useState<Mode>("agenda");
  const [events, setEvents] = useState<AgendaEvent[]>([]);
  const [error, setError] = useState<string>("");

  const window = useMemo(() => {
    const from = new Date(anchor.getFullYear(), anchor.getMonth(), 1).getTime();
    const to = new Date(anchor.getFullYear(), anchor.getMonth() + 1, 0, 23, 59, 59, 999).getTime();
    return { from, to };
  }, [anchor]);

  const refresh = useCallback(async (): Promise<void> => {
    try {
      setEvents(await api.calendarAgenda(window.from, window.to));
    } catch (e: unknown) {
      setError(String(e));
    }
  }, [window.from, window.to]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    const unlisten = onAppEvent((ev) => {
      if (ev.type === "TaskChanged" || ev.type === "ReminderScheduled" || ev.type === "ArtifactReady") {
        void refresh();
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refresh]);

  const byDay = useMemo(() => {
    const map = new Map<string, AgendaEvent[]>();
    for (const e of events) {
      const key = ymd(e.start_ms);
      const list = map.get(key) ?? [];
      list.push(e);
      map.set(key, list);
    }
    return map;
  }, [events]);

  const step = (delta: number): void => {
    setAnchor((a) => new Date(a.getFullYear(), a.getMonth() + delta, 1));
  };

  const exportIcs = useCallback(async (): Promise<void> => {
    try {
      const ics = await api.calendarExportIcs(window.from, window.to);
      const blob = new Blob([ics], { type: "text/calendar" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `casual-note-${anchor.getFullYear()}-${two(anchor.getMonth() + 1)}.ics`;
      document.body.appendChild(a);
      a.click();
      a.remove();
      URL.revokeObjectURL(url);
    } catch (e: unknown) {
      setError(String(e));
    }
  }, [anchor, window.from, window.to]);

  const open = (e: AgendaEvent): void => {
    if (e.source === "task" || e.source === "meeting") onOpenSource?.(e.source, e.source_id);
  };

  return (
    <div className="cal">
      <div className="cal-toolbar">
        <div className="cal-nav">
          <button type="button" className="btn btn-ghost" onClick={() => step(-1)} aria-label="Previous month">
            ‹
          </button>
          <span className="cal-month">
            {MONTHS[anchor.getMonth()]} {anchor.getFullYear()}
          </span>
          <button type="button" className="btn btn-ghost" onClick={() => step(1)} aria-label="Next month">
            ›
          </button>
        </div>
        <div className="cal-modes" role="tablist" aria-label="Calendar view">
          <button
            type="button"
            role="tab"
            aria-selected={mode === "agenda"}
            className={`cal-mode${mode === "agenda" ? " active" : ""}`}
            onClick={() => setMode("agenda")}
          >
            Agenda
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={mode === "month"}
            className={`cal-mode${mode === "month" ? " active" : ""}`}
            onClick={() => setMode("month")}
          >
            Month
          </button>
        </div>
        <button type="button" className="btn" onClick={() => void exportIcs()}>
          Export .ics
        </button>
      </div>

      {error && (
        <div className="error-banner" role="alert">
          {error}
        </div>
      )}

      <div className="cal-body">
        {mode === "agenda" ? (
          <AgendaList byDay={byDay} onOpen={open} />
        ) : (
          <MonthGrid anchor={anchor} byDay={byDay} onOpen={open} />
        )}
      </div>
    </div>
  );
}

function sourceLabel(source: AgendaEvent["source"]): string {
  return source === "meeting" ? "Meeting" : source === "reminder" ? "Reminder" : "Task";
}

interface AgendaListProps {
  byDay: Map<string, AgendaEvent[]>;
  onOpen: (e: AgendaEvent) => void;
}

function AgendaList({ byDay, onOpen }: AgendaListProps): React.JSX.Element {
  const days = [...byDay.keys()].sort();
  if (days.length === 0) {
    return <p className="cal-empty">Nothing scheduled this month.</p>;
  }
  const today = todayKey();
  return (
    <div className="cal-agenda">
      {days.map((key) => (
        <section key={key} className={`cal-day${key === today ? " today" : ""}`}>
          <div className="cal-day-head">
            <span className="cal-day-label">{dayHeading(key)}</span>
            {key === today && <span className="cal-today-pill">Today</span>}
          </div>
          <ul className="cal-events">
            {(byDay.get(key) ?? []).map((e) => (
              <li key={e.uid}>
                <button
                  type="button"
                  className={`cal-event src-${e.source}${e.status === "cancelled" ? " cancelled" : ""}`}
                  onClick={() => onOpen(e)}
                >
                  <span className="cal-event-time">{e.all_day ? "All day" : timeLabel(e.start_ms)}</span>
                  <span className="cal-event-main">
                    <span className="cal-event-title">{e.title}</span>
                    {e.location && <span className="cal-event-loc">{e.location}</span>}
                  </span>
                  <span className={`cal-tag src-${e.source}`}>{sourceLabel(e.source)}</span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

interface MonthGridProps {
  anchor: Date;
  byDay: Map<string, AgendaEvent[]>;
  onOpen: (e: AgendaEvent) => void;
}

function MonthGrid({ anchor, byDay, onOpen }: MonthGridProps): React.JSX.Element {
  const year = anchor.getFullYear();
  const month = anchor.getMonth();
  const firstWeekday = new Date(year, month, 1).getDay();
  const gridStart = new Date(year, month, 1 - firstWeekday);
  const today = todayKey();

  const cells: Date[] = [];
  for (let i = 0; i < 42; i += 1) {
    cells.push(new Date(gridStart.getFullYear(), gridStart.getMonth(), gridStart.getDate() + i));
  }

  return (
    <div className="cal-grid" role="grid">
      {WEEKDAYS.map((w) => (
        <div key={w} className="cal-grid-weekday" role="columnheader">
          {w}
        </div>
      ))}
      {cells.map((d) => {
        const key = `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}`;
        const dayEvents = byDay.get(key) ?? [];
        const outside = d.getMonth() !== month;
        return (
          <div
            key={key}
            role="gridcell"
            className={`cal-cell${outside ? " outside" : ""}${key === today ? " today" : ""}`}
          >
            <span className="cal-cell-num">{d.getDate()}</span>
            <div className="cal-cell-events">
              {dayEvents.slice(0, 3).map((e) => (
                <button
                  key={e.uid}
                  type="button"
                  className={`cal-pill src-${e.source}`}
                  onClick={() => onOpen(e)}
                  title={`${e.title}${e.location ? ` — ${e.location}` : ""}`}
                >
                  {e.title}
                </button>
              ))}
              {dayEvents.length > 3 && <span className="cal-more">+{dayEvents.length - 3} more</span>}
            </div>
          </div>
        );
      })}
    </div>
  );
}
