# Casual Note — Calendar & System-Calendar Sync

*Fifth surface: a local calendar that unifies the user's schedule with their notes, tasks, reminders, and meetings — and syncs, privately, with the calendar the user already has.*

**Status:** Downstream design doc, governed by the Design Foundation. This document **supersedes the earlier "no calendar/email integration" v1 non-goal** (Architecture §non-goals / Roadmap Phase 4): calendar is promoted to a first-class, near-term surface at the product owner's direction. Two-way *email* integration remains out of scope.

---

## 1. Why calendar, and the privacy stance

Time is the one axis notes, tasks, reminders, and meetings all share. A task has a due date; a reminder fires at a time; a meeting happens in a slot. Casual Note already owns all of that data locally — a calendar view is the natural unification, and syncing it to the user's system calendar makes Casual Note items show up next to the rest of their life (and vice-versa).

The privacy promise is unchanged and **binds the calendar too**:

- Calendar data lives in the same encrypted local store. A calendar view works fully offline.
- Sync targets **only the user's own calendar** — a native system calendar account or a CalDAV server *they* configure. Casual Note runs **no calendar server** and sends data to **no third party**.
- Sync is **opt-in per calendar**, and credentials live in the OS keystore (never on disk in plaintext).
- Only the `sync` path (and the existing `model-download`/`updater`) may open a socket, and only after the user connects an account. Everything else is local.

## 2. What it does

- **Unified agenda / calendar view** (month / week / day / agenda) that overlays, on one timeline: system-calendar events, meetings, tasks with scheduled/due dates, and reminders.
- **Two-way sync** with the user's system calendar so Casual Note items can appear there and external events appear here.
- **Project Casual Note items to events** (opt-in, per type): a scheduled task → an event/all-day item; a reminder → an event with a `VALARM`; a meeting → an event linked back to its transcript and notes.
- **Create/edit native events** from inside Casual Note (on writable calendars).
- **ICS import/export** for one-off interchange and offline transfer.

## 3. Sync architecture — three tiers, honest capability reporting

There is no single cross-platform "the system calendar" API, so — exactly like the audio-capture subsystem — calendar sync is a **capability-tiered adapter** behind one trait, and the UI reports the truth per platform.

| Tier | Mechanism | Platforms | Capability |
|------|-----------|-----------|-----------|
| **A — Native** | OS calendar store | macOS **EventKit** (`EKEventStore`); Linux **Evolution-Data-Server** (D-Bus) / GNOME Online Accounts; Windows **AppointmentManager** (read + limited write) | Full where the OS exposes it; permission-gated |
| **B — CalDAV** (baseline) | RFC 4791 + RFC 6578 sync-collection over the user's own server (iCloud, Google, Fastmail, Nextcloud, …) | all | Full two-way; the universal path |
| **C — ICS** | RFC 5545 file import/export | all | One-way interchange; always available |

**Baseline = CalDAV + ICS** (works everywhere, no native FFI). Native (Tier A) is an enhancement added per-OS; where it isn't available or permitted, the UI says so and offers CalDAV/ICS instead — never a silent downgrade.

### 3.1 Sync trait sketch

```rust
pub trait CalendarSyncAdapter {
    fn capability(&self) -> CalendarCapability;         // { read, write, push, tier, needs_permission }
    async fn list_calendars(&self) -> Result<Vec<RemoteCalendar>, SyncError>;
    async fn pull(&self, cal: &CalId, since: SyncToken)  // incremental: CalDAV sync-token / native change token
        -> Result<ChangeSet, SyncError>;
    async fn push(&self, cal: &CalId, ops: &[EventOp])   // create/update/delete on a writable calendar
        -> Result<Vec<PushResult>, SyncError>;
}

pub enum CalendarCapability {
    Native  { read: bool, write: bool },
    CalDav  { read: bool, write: bool },
    IcsOnly,
    Unavailable,
}
```

### 3.2 Conflict resolution

Two-way sync means conflicts. The rules:

- **Identity** by iCalendar `UID`; version by `SEQUENCE` + `LAST-MODIFIED`; CalDAV concurrency via **`ETag`** with `If-Match` preconditions.
- **Last-writer-wins by `LAST-MODIFIED`**, but a losing *local* edit is never discarded silently — it is retained as a revision and surfaced for review (consistent with the note "user edit is authoritative" rule).
- Recurrence uses the shared `rrule` engine; overrides are `RECURRENCE-ID` exceptions.
- Pull is incremental (sync-token / ctag); a full resync is available and idempotent.

## 4. Data model additions

New tables on the existing entity spine (details owned by the Data Model doc once ratified; sketch here):

- **`calendar`** — `id, name, source(system|caldav|local), account_ref, color, writable, tz, sync_token, ctag, enabled`.
- **`event`** — `id (entity), calendar_id, uid, title, start_utc, end_utc, all_day, tz, rrule, location, description, status, transparency, sequence, last_modified, etag, source_ref` (nullable link to the task/reminder/meeting it was projected from).
- **`calendar_account`** — connection metadata; **secrets live in the OS keystore, never here**.
- Sync bookkeeping reuses the `entity_op` op-log + a per-calendar sync journal, so calendar state is crash-safe and rebuildable like everything else.

Cross-pillar links use the existing polymorphic `link` table: `event ↔ task | reminder | session(meeting) | note`, so clicking an event opens the meeting's transcript or the task it came from.

## 5. Projection: Casual Note items → events

Opt-in per type, non-destructive (a projected event carries `source_ref` and is regenerated, never hand-edited into divergence):

| Source | Becomes | Notes |
|--------|---------|-------|
| Task with `start_on`/`deadline_on` | timed or all-day event | completing the task updates/removes the event |
| Reminder | event + `VALARM` | recurrence via the same RRULE |
| Meeting (session) | event spanning the recording | linked to transcript + artifacts |

The reverse — an external event with the app's marker — can spawn a Casual Note task/note on request.

## 6. UX

- **Calendar view**: month / week / day / agenda; Casual Note items and system events visually distinguished; toggle which calendars are shown.
- **Account setup**: "Connect system calendar" (native permission prompt) or "Add CalDAV account" (server URL + credentials → OS keystore) or "Import .ics".
- **Capability honesty banner** per platform (e.g. "Windows: read-only native calendar; connect CalDAV for two-way sync").
- **Create/edit event** inline; drag to reschedule; link an event to a note/task/meeting.

## 7. Security & privacy specifics

- CalDAV over TLS to the user's own server only; credentials in Keychain / Credential Manager / Secret Service.
- No calendar data leaves the device except to the calendar server the user explicitly connected.
- Sync is auditable and disable-per-calendar; a global "pause sync" honors offline mode.
- The telemetry-absence guarantee holds: calendar network access is confined to the `calendar` sync path and is covered by the offline-network CI job.

## 8. Delivery — new workstream & phase

- **Workstream W13 — Calendar & Sync**: `calendar` crate (domain, ICS, CalDAV, sync trait + adapters), `ui/calendar`, app-service wiring, capability reporting.
- **Phasing**: the **`calendar` engine crate** (domain model, RFC 5545 ICS import/export, RFC 4791 CalDAV two-way sync, the sync trait + a working CalDAV/ICS path, Linux EDS + macOS EventKit + Windows adapters behind a documented FFI seam with honest capability reporting) is built as a standalone, disjoint unit. The **app-service/UI wiring and the Calendar view** land after the Phase-2 meeting pipeline (M2) so they attach to a stable command/event surface. Native FFI adapters (EventKit/EDS) follow the same "engine trait first, native backend second" pattern used for audio capture and STT/LLM.

## 9. Acceptance gates

- ICS export→import round-trips a representative event corpus (recurrence, all-day, tz, alarms) losslessly.
- CalDAV two-way sync against a test server: create/update/delete propagate both ways; incremental sync-token pull; **ETag conflict is detected and the losing local edit is preserved**, never dropped.
- Full offline: the calendar view and all local editing work with the network disabled; only a connected account's sync opens a socket.
- Capability report matches reality per OS; no silent downgrade.
