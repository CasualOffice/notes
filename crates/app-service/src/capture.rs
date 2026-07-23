//! Quick-capture + NLP preview (HLD §8.2, Feature Specs §2). `nlp.parse` is a pure
//! preview (no writes); `capture.quick` routes the parsed entry to a task / note /
//! reminder via the same workflows the explicit commands use.

use app_domain::{AppError, AppResult, EntityKind, EntityRef};
use app_nlp::{ParseContext, ParsedEntry, Route};

use crate::dto::{CaptureResult, NewReminder, NewTask};
use crate::notes::parse_id;
use crate::Service;

/// Build a parse context anchored at the OS-local "now" (offset-aware for correct
/// local→UTC conversion). The IANA zone name is a passthrough; Phase-1 uses
/// `"local"` (a full IANA resolver is a later refinement — see return notes).
fn local_context() -> ParseContext {
    let now = chrono::Local::now().fixed_offset();
    ParseContext::new(now, "local")
}

impl Service {
    /// `nlp.parse` — live-highlight preview, no side effects (HLD §8.2).
    pub fn nlp_parse(&self, text: &str) -> AppResult<ParsedEntry> {
        let ctx = local_context();
        Ok(app_nlp::parse(text, &ctx).entry)
    }

    /// `capture.quick` — parse, route, and persist. `kind_hint` overrides the parsed
    /// route (AC-2.4).
    pub fn capture_quick(&self, text: &str, kind_hint: Option<String>) -> AppResult<CaptureResult> {
        let ctx = local_context();
        let parsed = app_nlp::parse(text, &ctx).entry;

        let route = match kind_hint.as_deref() {
            Some("task") => Route::Task,
            Some("note") => Route::Note,
            Some("reminder") => Route::Reminder,
            _ => parsed.kind,
        };

        let entity_ref = match route {
            Route::Task => {
                let new = NewTask {
                    title: parsed.title.clone(),
                    project_id: None,
                    area_id: None,
                    notes_md: None,
                    start_on: parsed.start_on.map(|d| d.to_string()),
                    deadline_on: parsed.deadline_on.map(|d| d.to_string()),
                    someday: None,
                    priority: Some(i64::from(parsed.priority)),
                };
                let view = self.tasks_create(new)?;
                EntityRef::new(EntityKind::Task, parse_id(&view.id)?)
            }
            Route::Reminder => {
                let spec = parsed.reminder.clone().ok_or_else(|| {
                    AppError::Nlp("reminder route without a resolved fire time".into())
                })?;
                let new = NewReminder {
                    target: None,
                    fire_at: spec.fire_at.as_millis(),
                    tz: spec.tz,
                    body: Some(parsed.title.clone()),
                };
                let (id, _sched) = self.reminders_create(new)?;
                EntityRef::new(EntityKind::Reminder, parse_id(&id)?)
            }
            Route::Note => {
                let doc = serde_json::json!({
                    "type": "doc",
                    "content": [{
                        "type": "paragraph",
                        "content": [{ "type": "text", "text": parsed.title }]
                    }]
                })
                .to_string();
                let id = self.notes_create(None, None, Some(doc))?;
                EntityRef::new(EntityKind::Note, parse_id(&id)?)
            }
        };

        Ok(CaptureResult { entity_ref, parsed })
    }
}
