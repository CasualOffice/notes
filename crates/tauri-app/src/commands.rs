//! The public `#[tauri::command]` surface — the only WebView↔Core door (HLD §6).
//!
//! Every command validates/deserializes its arguments, delegates to the
//! `app-service` facade (which owns transactions + `AppEvent` emission), and returns
//! `Result<T, AppError>` (the stable `{class, retryable, message}` wire shape).
//!
//! **Naming:** HLD command names use dots (`notes.save`); Rust identifiers can't, so
//! each maps to its underscore form (`notes.save` → `notes_save`). The WebView
//! `invoke`s the underscore name. Later-phase commands (meeting / AI / models /
//! export) return a typed "not implemented in this phase" error via
//! [`app_service::stubs`].

use std::sync::Arc;

use app_domain::{AppError, Bucket, EntityRef, Platform, PlatformCaps};
use app_service::dto::{
    BacklinkRef, BacklinkView, BlockView, CaptureResult, LinkResolution, NewReminder, NewTask,
    Note, NoteSummary, NoteView, NotebookNode, ReminderView, SaveResult, SearchResultsDto,
    TaskPatch, TaskView, UnlinkedMention,
};
use app_service::{stubs, ParsedEntry, Service};
use tauri::State;

// `State` MUST appear literally in each command signature — the `#[tauri::command]`
// macro recognizes it by its path segment, so a type alias would be misread as a
// deserializable argument.

// --- Notes & blocks --------------------------------------------------------

#[tauri::command]
pub fn notes_create(
    service: State<'_, Arc<Service>>,
    notebook_id: Option<String>,
    daily_date: Option<String>,
    doc_json: Option<String>,
) -> Result<String, AppError> {
    service.notes_create(notebook_id, daily_date, doc_json)
}

#[tauri::command]
pub fn notes_get(service: State<'_, Arc<Service>>, note_id: String) -> Result<NoteView, AppError> {
    service.notes_get(&note_id)
}

#[tauri::command]
pub fn notes_save(
    service: State<'_, Arc<Service>>,
    note_id: String,
    doc_json: String,
    base_version: i64,
) -> Result<SaveResult, AppError> {
    service.notes_save(&note_id, &doc_json, base_version)
}

#[tauri::command]
pub fn notes_list(
    service: State<'_, Arc<Service>>,
    notebook_id: Option<String>,
) -> Result<Vec<NoteSummary>, AppError> {
    service.notes_list(notebook_id)
}

#[tauri::command]
pub fn notes_delete(service: State<'_, Arc<Service>>, note_id: String) -> Result<(), AppError> {
    service.notes_delete(&note_id)
}

#[tauri::command]
pub fn notes_resolve_links(
    service: State<'_, Arc<Service>>,
    note_id: String,
) -> Result<Vec<LinkResolution>, AppError> {
    service.notes_resolve_links(&note_id)
}

#[tauri::command]
pub fn blocks_get(
    service: State<'_, Arc<Service>>,
    block_id: String,
) -> Result<BlockView, AppError> {
    service.blocks_get(&block_id)
}

#[tauri::command]
pub fn blocks_backlinks(
    service: State<'_, Arc<Service>>,
    target: EntityRef,
) -> Result<Vec<BacklinkView>, AppError> {
    service.blocks_backlinks(target)
}

// --- Notebooks / folder tree (M1) ------------------------------------------

#[tauri::command]
pub fn notebooks_list(service: State<'_, Arc<Service>>) -> Result<Vec<NotebookNode>, AppError> {
    service.notebooks_list()
}

#[tauri::command]
pub fn notebooks_create(
    service: State<'_, Arc<Service>>,
    name: String,
    parent_id: Option<String>,
) -> Result<String, AppError> {
    service.notebooks_create(name, parent_id)
}

#[tauri::command]
pub fn notes_move(
    service: State<'_, Arc<Service>>,
    note_id: String,
    notebook_id: Option<String>,
) -> Result<NoteView, AppError> {
    service.notes_move(&note_id, notebook_id)
}

// --- Daily notes (M1) ------------------------------------------------------

#[tauri::command]
pub fn daily_get_or_create(
    service: State<'_, Arc<Service>>,
    date: String,
) -> Result<Note, AppError> {
    service.daily_get_or_create(&date)
}

// --- Backlinks (M1) --------------------------------------------------------

#[tauri::command]
pub fn links_backlinks(
    service: State<'_, Arc<Service>>,
    entity_id: String,
) -> Result<Vec<BacklinkRef>, AppError> {
    service.links_backlinks(&entity_id)
}

#[tauri::command]
pub fn links_unlinked_mentions(
    service: State<'_, Arc<Service>>,
    entity_id: String,
) -> Result<Vec<UnlinkedMention>, AppError> {
    service.links_unlinked_mentions(&entity_id)
}

// --- Markdown I/O (M1) -----------------------------------------------------

#[tauri::command]
pub fn notes_export_markdown(
    service: State<'_, Arc<Service>>,
    note_id: String,
) -> Result<String, AppError> {
    service.notes_export_markdown(&note_id)
}

#[tauri::command]
pub fn notes_import_markdown(
    service: State<'_, Arc<Service>>,
    md: String,
    notebook_id: Option<String>,
) -> Result<Note, AppError> {
    service.notes_import_markdown(&md, notebook_id)
}

// --- Tasks / projects / areas ----------------------------------------------

#[tauri::command]
pub fn tasks_create(
    service: State<'_, Arc<Service>>,
    input: NewTask,
) -> Result<TaskView, AppError> {
    service.tasks_create(input)
}

#[tauri::command]
pub fn tasks_update(
    service: State<'_, Arc<Service>>,
    task_id: String,
    patch: TaskPatch,
) -> Result<TaskView, AppError> {
    service.tasks_update(&task_id, patch)
}

#[tauri::command]
pub fn tasks_complete(
    service: State<'_, Arc<Service>>,
    task_id: String,
    at: Option<i64>,
) -> Result<TaskView, AppError> {
    service.tasks_complete(&task_id, at)
}

#[tauri::command]
pub fn tasks_reorder(
    service: State<'_, Arc<Service>>,
    task_id: String,
    before: Option<String>,
    after: Option<String>,
) -> Result<String, AppError> {
    service.tasks_reorder(&task_id, before, after)
}

#[tauri::command]
pub fn tasks_bucket(
    service: State<'_, Arc<Service>>,
    bucket: Bucket,
) -> Result<Vec<TaskView>, AppError> {
    service.tasks_bucket(bucket)
}

#[tauri::command]
pub fn projects_create(
    service: State<'_, Arc<Service>>,
    name: String,
    area_id: Option<String>,
) -> Result<String, AppError> {
    service.projects_create(name, area_id)
}

#[tauri::command]
pub fn areas_create(
    service: State<'_, Arc<Service>>,
    name: String,
    icon: Option<String>,
) -> Result<String, AppError> {
    service.areas_create(name, icon)
}

// --- Reminders -------------------------------------------------------------

#[tauri::command]
pub fn reminders_create(
    service: State<'_, Arc<Service>>,
    input: NewReminder,
) -> Result<String, AppError> {
    // The ScheduleRequest is emitted as ReminderScheduled and rebuilt into Layer A
    // on the next scheduler boot; live arming is a later-phase wiring.
    service.reminders_create(input).map(|(id, _sched)| id)
}

#[tauri::command]
pub fn reminders_snooze(
    service: State<'_, Arc<Service>>,
    reminder_id: String,
    until: i64,
) -> Result<ReminderView, AppError> {
    service.reminders_snooze(&reminder_id, until)
}

#[tauri::command]
pub fn reminders_cancel(
    service: State<'_, Arc<Service>>,
    reminder_id: String,
) -> Result<(), AppError> {
    service.reminders_cancel(&reminder_id)
}

#[tauri::command]
pub fn reminders_upcoming(
    service: State<'_, Arc<Service>>,
    horizon: Option<i64>,
) -> Result<Vec<ReminderView>, AppError> {
    service.reminders_upcoming(horizon)
}

// --- Quick capture & NLP ---------------------------------------------------

#[tauri::command]
pub fn capture_quick(
    service: State<'_, Arc<Service>>,
    text: String,
    kind_hint: Option<String>,
) -> Result<CaptureResult, AppError> {
    service.capture_quick(&text, kind_hint)
}

#[tauri::command]
pub fn nlp_parse(service: State<'_, Arc<Service>>, text: String) -> Result<ParsedEntry, AppError> {
    service.nlp_parse(&text)
}

// --- Search & palette ------------------------------------------------------

#[tauri::command]
pub fn search_query(
    service: State<'_, Arc<Service>>,
    q: String,
    mode: Option<String>,
    limit: Option<u32>,
) -> Result<SearchResultsDto, AppError> {
    service.search_query(&q, mode.as_deref().unwrap_or("go"), limit)
}

#[tauri::command]
pub fn palette_run(
    service: State<'_, Arc<Service>>,
    mode: String,
    input: String,
) -> Result<serde_json::Value, AppError> {
    service.palette_run(&mode, &input)
}

// --- System / capabilities -------------------------------------------------

/// Honest per-platform capability report (HLD §9, `CapabilityReport`). Pure and
/// side-effect-free: the WebView calls this once on boot so it never exposes a
/// capability the host lacks (e.g. Linux has no OS reminder-scheduling layer).
#[tauri::command]
pub fn get_capabilities() -> Result<PlatformCaps, AppError> {
    Ok(platform_caps())
}

/// The compile-target's honest capabilities. M0 ships no meeting-audio adapter, so
/// audio caps are `false` everywhere; reminder layering follows the platform (Linux
/// is running-only — HLD §9.3).
pub(crate) fn platform_caps() -> PlatformCaps {
    // `Platform::current()` is `None` only on unsupported targets; the desktop
    // binary only ever builds for one of the three, so fall back to Linux (the
    // most conservative capability profile) rather than panic.
    let platform = Platform::current().unwrap_or(Platform::Linux);
    let has_os_reminder_layer = matches!(platform, Platform::Macos | Platform::Windows);
    PlatformCaps {
        platform,
        app_audio_capture: false,
        exclude_self: false,
        reminder_os_layer: has_os_reminder_layer,
        reminder_running_only: !has_os_reminder_layer,
    }
}

// --- Later-phase stubs (typed "not implemented in this phase") --------------

#[tauri::command]
pub fn meeting_preflight() -> Result<serde_json::Value, AppError> {
    Err(stubs::not_implemented("meeting.preflight"))
}

#[tauri::command]
pub fn meeting_start() -> Result<serde_json::Value, AppError> {
    Err(stubs::not_implemented("meeting.start"))
}

#[tauri::command]
pub fn meeting_stop() -> Result<serde_json::Value, AppError> {
    Err(stubs::not_implemented("meeting.stop"))
}

#[tauri::command]
pub fn meeting_artifact() -> Result<serde_json::Value, AppError> {
    Err(stubs::not_implemented("meeting.artifact"))
}

#[tauri::command]
pub fn meeting_action_item_to_task() -> Result<serde_json::Value, AppError> {
    Err(stubs::not_implemented("meeting.actionItemToTask"))
}

#[tauri::command]
pub fn ai_ask() -> Result<serde_json::Value, AppError> {
    Err(stubs::not_implemented("ai.ask"))
}

#[tauri::command]
pub fn ai_suggestions_list() -> Result<serde_json::Value, AppError> {
    Err(stubs::not_implemented("ai.suggestions.list"))
}

#[tauri::command]
pub fn models_list() -> Result<serde_json::Value, AppError> {
    Err(stubs::not_implemented("models.list"))
}

#[tauri::command]
pub fn models_install() -> Result<serde_json::Value, AppError> {
    Err(stubs::not_implemented("models.install"))
}

#[tauri::command]
pub fn export_note() -> Result<serde_json::Value, AppError> {
    Err(stubs::not_implemented("export.note"))
}
