//! Notebook / folder-tree workflows (Data Model §4.3, Feature Specs §1.1). A
//! notebook is a spine entity (`kind='notebook'`) with a `notebook` detail row
//! carrying `parent_id`/`order_key`/`icon`/`color`. Nesting is by `parent_id`; the
//! tree is assembled derived-on-read. Notes join a notebook via `note.notebook_id`
//! (see [`Service::notes_move`]).

use app_domain::{AppError, AppEvent, AppResult, Id};
use rusqlite::{params, Connection};
use serde_json::Value;
use storage::DetailTable;

use crate::dto::{NoteView, NotebookNode};
use crate::notes::parse_id;
use crate::util::{self, Columns};
use crate::Service;

/// A flat notebook row read back for tree assembly.
struct FlatNotebook {
    id: Id,
    name: Option<String>,
    parent_id: Option<Id>,
    order_key: String,
    icon: Option<String>,
    color: Option<String>,
}

impl Service {
    /// `notebooks.list` — the full live notebook/folder tree, ordered by
    /// `order_key` then `created_at` at each level (Data Model §4.3).
    pub fn notebooks_list(&self) -> AppResult<Vec<NotebookNode>> {
        let flat: Vec<FlatNotebook> = self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT e.id, e.title, nb.parent_id, nb.order_key, nb.icon, nb.color \
                 FROM notebook nb JOIN entity e ON e.id = nb.entity_id \
                 WHERE e.deleted_at IS NULL \
                 ORDER BY nb.order_key, e.created_at",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(FlatNotebook {
                        id: Id::from_bytes(to16(&r.get::<_, Vec<u8>>(0)?)),
                        name: r.get(1)?,
                        parent_id: r
                            .get::<_, Option<Vec<u8>>>(2)?
                            .map(|b| Id::from_bytes(to16(&b))),
                        order_key: r.get(3)?,
                        icon: r.get(4)?,
                        color: r.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        Ok(build_tree(flat))
    }

    /// `notebooks.create` — a new notebook, optionally nested under `parent_id`
    /// (which must itself be a live notebook). Ordered after its last sibling.
    pub fn notebooks_create(&self, name: String, parent_id: Option<String>) -> AppResult<String> {
        let id = Id::new();
        let now = self.now_ms();

        // Validate the parent is a live notebook before threading the child under it.
        let parent = match parent_id {
            Some(p) => {
                let pid = parse_id(&p)?;
                let spine = self
                    .read(|c| Ok(util::read_spine(c, pid)?))?
                    .ok_or_else(|| AppError::NotFound(format!("notebook {p}")))?;
                if spine.kind != "notebook" {
                    return Err(AppError::Validation(format!("{p} is not a notebook")));
                }
                Some(pid)
            }
            None => None,
        };

        let order_key = self.next_notebook_key(parent)?;
        let mut cols = Columns::new();
        cols.insert("order_key".into(), Value::String(order_key));
        if let Some(pid) = parent {
            cols.insert("parent_id".into(), Value::String(pid.to_string()));
        }

        let title = (!name.trim().is_empty()).then(|| name.trim().to_string());
        self.commit(&util::create_op(
            id,
            self.next_hlc(),
            "notebook",
            title,
            None,
            now,
            Some((DetailTable::Notebook, cols)),
        ))?;
        // No dedicated notebook AppEvent exists; the WebView refreshes the tree by
        // re-reading `notebooks.list` after a successful create.
        Ok(id.to_string())
    }

    /// Move a note into a notebook (or to the top level when `notebook_id` is
    /// `None`). Updates `note.notebook_id`; the body is untouched. Emits
    /// [`AppEvent::NoteSaved`] with no changed blocks so the UI refreshes meta.
    pub fn notes_move(&self, note_id: &str, notebook_id: Option<String>) -> AppResult<NoteView> {
        let id = parse_id(note_id)?;
        let now = self.now_ms();
        let spine = self
            .read(|c| Ok(util::read_spine(c, id)?))?
            .ok_or_else(|| AppError::NotFound(format!("note {note_id}")))?;
        if spine.kind != "note" {
            return Err(AppError::Validation(format!("{note_id} is not a note")));
        }

        let target = match notebook_id {
            Some(nb) => {
                let nbid = parse_id(&nb)?;
                let nb_spine = self
                    .read(|c| Ok(util::read_spine(c, nbid)?))?
                    .ok_or_else(|| AppError::NotFound(format!("notebook {nb}")))?;
                if nb_spine.kind != "notebook" {
                    return Err(AppError::Validation(format!("{nb} is not a notebook")));
                }
                Value::String(nbid.to_string())
            }
            // Clearing the FK sends a JSON null → SQL NULL (top-level note).
            None => Value::Null,
        };

        self.commit(&util::update_op(
            id,
            self.next_hlc(),
            &spine,
            None,
            now,
            Some((DetailTable::Note, util::col1("notebook_id", target))),
        ))?;
        self.emit(AppEvent::NoteSaved {
            note_id: id,
            version: now as u64,
            changed_block_ids: Vec::new(),
        });
        self.notes_get(note_id)
    }

    /// The `order_key` for a new notebook: after the current max among its siblings
    /// (same `parent_id`), or the initial key for the first sibling.
    fn next_notebook_key(&self, parent: Option<Id>) -> AppResult<String> {
        let max: Option<String> = self.read(move |c: &Connection| {
            let row = match parent {
                Some(pid) => c
                    .query_row(
                        "SELECT nb.order_key FROM notebook nb JOIN entity e ON e.id = nb.entity_id \
                         WHERE e.deleted_at IS NULL AND nb.parent_id = ?1 \
                         ORDER BY nb.order_key DESC LIMIT 1",
                        params![pid.as_bytes().as_slice()],
                        |r| r.get::<_, String>(0),
                    )
                    .ok(),
                None => c
                    .query_row(
                        "SELECT nb.order_key FROM notebook nb JOIN entity e ON e.id = nb.entity_id \
                         WHERE e.deleted_at IS NULL AND nb.parent_id IS NULL \
                         ORDER BY nb.order_key DESC LIMIT 1",
                        [],
                        |r| r.get::<_, String>(0),
                    )
                    .ok(),
            };
            Ok(row)
        })?;
        Ok(match max {
            Some(k) => tasks::key_after(&k).map_err(AppError::from)?,
            None => tasks::order_key::initial_key(),
        })
    }
}

/// Assemble the flat rows into a forest, preserving the input order (already sorted
/// by `order_key, created_at`) at every level. Orphans (a `parent_id` pointing at a
/// missing/deleted notebook) surface at the top level so nothing is lost.
fn build_tree(flat: Vec<FlatNotebook>) -> Vec<NotebookNode> {
    use std::collections::HashMap;

    let ids: std::collections::HashSet<Id> = flat.iter().map(|f| f.id).collect();
    // Preserve deterministic order via the pre-sorted input.
    let mut children_of: HashMap<Option<Id>, Vec<Id>> = HashMap::new();
    let mut by_id: HashMap<Id, FlatNotebook> = HashMap::new();
    for f in flat {
        let parent = match f.parent_id {
            Some(p) if ids.contains(&p) => Some(p),
            _ => None, // orphan → top level
        };
        children_of.entry(parent).or_default().push(f.id);
        by_id.insert(f.id, f);
    }

    fn assemble(
        id: Id,
        by_id: &std::collections::HashMap<Id, FlatNotebook>,
        children_of: &std::collections::HashMap<Option<Id>, Vec<Id>>,
    ) -> NotebookNode {
        let f = &by_id[&id];
        let children = children_of
            .get(&Some(id))
            .map(|kids| {
                kids.iter()
                    .map(|k| assemble(*k, by_id, children_of))
                    .collect()
            })
            .unwrap_or_default();
        NotebookNode {
            id: f.id.to_string(),
            name: f.name.clone(),
            parent_id: f.parent_id.map(|p| p.to_string()),
            order_key: f.order_key.clone(),
            icon: f.icon.clone(),
            color: f.color.clone(),
            children,
        }
    }

    children_of
        .get(&None)
        .map(|roots| {
            roots
                .iter()
                .map(|r| assemble(*r, &by_id, &children_of))
                .collect()
        })
        .unwrap_or_default()
}

fn to16(b: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let n = b.len().min(16);
    out[..n].copy_from_slice(&b[..n]);
    out
}
