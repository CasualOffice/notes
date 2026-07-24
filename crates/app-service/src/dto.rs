//! Command request/response DTOs — the typed wire shapes the WebView exchanges
//! with the Rust core (HLD §6). These are `app-service`-owned because app-domain is
//! the dependency-light vocabulary and deliberately carries no command payloads;
//! all ids cross the boundary as hyphenated-UUID strings.

use app_domain::EntityRef;
use app_nlp::ParsedEntry;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Notes & blocks
// ---------------------------------------------------------------------------

/// The M0 note aggregate returned by the `create_note` / `get_note` /
/// `update_note` use cases (HLD note-create sequence): spine identity plus the
/// source-of-truth `doc_json`. A lean shape distinct from the fuller
/// [`NoteView`] used by the Phase-1 `notes.*` command surface.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub title: Option<String>,
    pub doc_json: String,
    /// Optimistic-concurrency token (== `entity.updated_at` ms).
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// `notes.get` result: the source-of-truth `doc_json` plus spine/detail meta.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoteView {
    pub id: String,
    pub title: Option<String>,
    pub doc_json: String,
    pub notebook_id: Option<String>,
    pub daily_date: Option<String>,
    pub is_pinned: bool,
    /// Optimistic-concurrency token (== `entity.updated_at` ms).
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// `notes.save` result (HLD §6).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SaveResult {
    /// New optimistic-concurrency token.
    pub version: i64,
    pub changed_block_ids: Vec<String>,
}

/// `notes.list` element.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoteSummary {
    pub id: String,
    pub title: Option<String>,
    pub daily_date: Option<String>,
    pub is_pinned: bool,
    pub updated_at: i64,
}

/// `blocks.get` result (Data Model §4.2 projected block).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockView {
    pub block_id: String,
    pub note_id: String,
    pub node_type: String,
    pub seq: i64,
    pub depth: i64,
    pub text_content: Option<String>,
    pub order_key: String,
}

/// `blocks.backlinks` element (derived-on-read from `links`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BacklinkView {
    pub src_entity: String,
    pub src_kind: String,
    pub src_title: Option<String>,
    pub src_block_id: Option<String>,
    pub rel: String,
}

/// `notes.resolveLinks` element: a wikilink target and how it resolved.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkResolution {
    pub target_title: String,
    pub rel: String,
    pub resolved_id: Option<String>,
    /// True when a stub note was created for an unresolved `[[X]]` (HLD §8.1).
    pub created_stub: bool,
}

// ---------------------------------------------------------------------------
// Notebooks / folder tree (M1)
// ---------------------------------------------------------------------------

/// One node in the `notebooks.list` tree (Data Model §4.3). A notebook is a spine
/// entity (`kind='notebook'`); `children` are its live sub-notebooks, recursively.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotebookNode {
    pub id: String,
    /// The notebook's display name (spine `title`).
    pub name: Option<String>,
    pub parent_id: Option<String>,
    pub order_key: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub children: Vec<NotebookNode>,
}

// ---------------------------------------------------------------------------
// Backlinks (M1 `links.backlinks` / `links.unlinked_mentions`)
// ---------------------------------------------------------------------------

/// A resolved reverse reference to a target entity — the "Linked mentions" panel
/// row (Feature Specs §1.2). Derived-on-read from `link`; never materialized.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BacklinkRef {
    pub source_note_id: String,
    pub source_title: Option<String>,
    /// The precise origin block (`link.src_block_id`), for the jump affordance.
    pub block_id: Option<String>,
    /// The surrounding block text, truncated for the panel.
    pub snippet: String,
}

/// An "unlinked mention" — a note whose text matches the target's title via FTS but
/// that carries no `wikilink`/`mention` edge to it yet (Feature Specs §1.2). Surfaced
/// live, never stored.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnlinkedMention {
    pub source_note_id: String,
    pub source_title: Option<String>,
    /// A snippet of the matching note's body around the title occurrence.
    pub snippet: String,
}

// ---------------------------------------------------------------------------
// Tasks / projects / areas
// ---------------------------------------------------------------------------

/// The task detail + spine projection returned by the task commands.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskView {
    pub id: String,
    pub title: Option<String>,
    pub project_id: Option<String>,
    pub area_id: Option<String>,
    pub notes_md: Option<String>,
    pub status: String,
    pub priority: i64,
    pub someday: bool,
    pub start_on: Option<String>,
    pub deadline_on: Option<String>,
    pub completed_at: Option<i64>,
    pub order_key: String,
}

/// `tasks.create` input.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct NewTask {
    pub title: String,
    pub project_id: Option<String>,
    pub area_id: Option<String>,
    pub notes_md: Option<String>,
    pub start_on: Option<String>,
    pub deadline_on: Option<String>,
    pub someday: Option<bool>,
    pub priority: Option<i64>,
}

/// `tasks.update` patch — every field optional (per-field LWW at the writer).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub notes_md: Option<String>,
    pub status: Option<String>,
    pub priority: Option<i64>,
    pub someday: Option<bool>,
    pub start_on: Option<String>,
    pub deadline_on: Option<String>,
    pub project_id: Option<String>,
    pub area_id: Option<String>,
}

/// A `project` row projection (`tasks.projects_areas`). Projects group tasks and
/// optionally roll up under an [`AreaView`] (Data Model §6).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectView {
    pub id: String,
    pub name: Option<String>,
    pub area_id: Option<String>,
    pub status: String,
    pub order_key: String,
}

/// An `area` row projection (`tasks.projects_areas`). Areas are the top-level
/// spheres-of-responsibility buckets (Data Model §6).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AreaView {
    pub id: String,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub order_key: String,
}

/// `tasks.projects_areas` result — the sidebar's project/area index used to file
/// and group tasks (Feature Specs §3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectsAreas {
    pub projects: Vec<ProjectView>,
    pub areas: Vec<AreaView>,
}

// ---------------------------------------------------------------------------
// Calendar agenda (read-mostly projection — no new storage tables)
// ---------------------------------------------------------------------------

/// One row of the unified agenda (`calendar.agenda`): a task / reminder / meeting
/// projected into a calendar event via the `calendar` crate's projection helpers
/// (calendar §5). Read-mostly — never persisted; re-projected from live pillar
/// rows on every call.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgendaEvent {
    /// Deterministic projected `UID` (`<source>:<uuid>@casual-note`) — stable
    /// across re-projection, so the UI can dedupe/key on it.
    pub uid: String,
    /// Event `SUMMARY`.
    pub title: String,
    /// Start instant, absolute UTC epoch-ms.
    pub start_ms: i64,
    /// End instant, absolute UTC epoch-ms (exclusive for all-day).
    pub end_ms: i64,
    /// Whether this is an all-day (DATE-valued) event.
    pub all_day: bool,
    /// Which pillar produced the event: `task` | `reminder` | `meeting`.
    pub source: String,
    /// The originating pillar entity id (the task / reminder / session), for the
    /// jump-to-source affordance.
    pub source_id: String,
    /// iCalendar `STATUS` (`confirmed` | `tentative` | `cancelled`).
    pub status: String,
    /// `LOCATION`, when present.
    pub location: Option<String>,
    /// `DESCRIPTION`, when present.
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// Reminders
// ---------------------------------------------------------------------------

/// A `reminder` row projection returned by the reminder commands.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReminderView {
    pub id: String,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub fire_at: i64,
    pub tz: String,
    pub state: String,
    pub snoozed_until: Option<i64>,
    pub body: Option<String>,
}

/// `reminders.create` input.
#[derive(Clone, Debug, Deserialize)]
pub struct NewReminder {
    /// Target entity ref (`{kind, id}`); omitted for a standalone reminder.
    pub target: Option<EntityRef>,
    pub fire_at: i64,
    pub tz: String,
    pub body: Option<String>,
}

/// A scheduling descriptor the host lifts into `scheduler::ScheduledReminder` to
/// arm Layer A (kept scheduler-free so app-service doesn't depend on `scheduler`).
#[derive(Clone, Debug)]
pub struct ScheduleRequest {
    pub reminder_id: String,
    pub fire_at: i64,
    pub tz: String,
    pub body: Option<String>,
    pub target: Option<EntityRef>,
    /// Whether the platform has an OS one-shot layer (false on Linux — HLD §9.3).
    pub os_layer: bool,
}

// ---------------------------------------------------------------------------
// Quick capture & search
// ---------------------------------------------------------------------------

/// `capture.quick` result (HLD §6): what got written + the parse that routed it.
#[derive(Clone, Debug, Serialize)]
pub struct CaptureResult {
    pub entity_ref: EntityRef,
    pub parsed: ParsedEntry,
}

/// One `search.query` hit (a flattened, WebView-friendly `SearchHit`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchHitDto {
    pub kind: String,
    pub id: String,
    pub title: Option<String>,
    pub snippet: String,
    pub bm25: f64,
}

/// `search.query` result envelope.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResultsDto {
    pub query_id: String,
    pub hits: Vec<SearchHitDto>,
    pub complete: bool,
}
