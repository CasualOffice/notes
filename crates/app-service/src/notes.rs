//! Note create/edit, projection, and backlink resolution (HLD §8.1). `doc_json`
//! (Tiptap) is truth; `block`, projected `link` rows, and FTS are derived on save
//! and expressed as op-log entries so they rebuild bit-identically from the log.

use app_domain::{AppError, AppEvent, AppResult, BlockId, EntityKind, EntityRef, Id, LinkRel};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use storage::{BlockRow, DetailTable, LinkRow, OpBody};

use crate::dto::{
    BacklinkRef, BacklinkView, BlockView, LinkResolution, Note, NoteSummary, NoteView, SaveResult,
    UnlinkedMention,
};
use crate::util::{self, Columns};
use crate::Service;

/// Parse a hyphenated-UUID string into an [`Id`], failing as a validation error.
pub(crate) fn parse_id(s: &str) -> AppResult<Id> {
    s.parse::<Id>()
        .map_err(|_| AppError::Validation(format!("invalid id: {s}")))
}

struct NoteRow {
    doc_json: String,
    content_hash: Option<String>,
    notebook_id: Option<Vec<u8>>,
    daily_date: Option<String>,
    is_pinned: bool,
    title: Option<String>,
    created_at: i64,
    updated_at: i64,
}

fn read_note(conn: &Connection, id: Id) -> rusqlite::Result<Option<NoteRow>> {
    conn.query_row(
        "SELECT n.doc_json, n.content_hash, n.notebook_id, n.daily_date, n.is_pinned, \
                e.title, e.created_at, e.updated_at \
         FROM note n JOIN entity e ON e.id = n.entity_id \
         WHERE e.id = ?1 AND e.deleted_at IS NULL",
        params![id.as_bytes().as_slice()],
        |r| {
            Ok(NoteRow {
                doc_json: r.get(0)?,
                content_hash: r.get(1)?,
                notebook_id: r.get(2)?,
                daily_date: r.get(3)?,
                is_pinned: r.get::<_, i64>(4)? != 0,
                title: r.get(5)?,
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
            })
        },
    )
    .optional()
}

/// An empty Tiptap document (`doc` with one empty paragraph).
fn empty_doc() -> String {
    r#"{"type":"doc","content":[{"type":"paragraph"}]}"#.to_string()
}

impl Service {
    // -- M0 walking-skeleton use cases --------------------------------------
    //
    // The minimal, title-first note surface exercised by M0 (HLD note-create
    // sequence). These compose the same op-log write path as the Phase-1
    // `notes.*` commands, so every mutation appends to `entity_op` and the
    // derived tables stay rebuildable from the log (CLAUDE.md op-log invariant).

    /// M0: create a note with an explicit `title` and optional Tiptap `doc_json`
    /// (defaults to an empty document). In one logical write it allocates a
    /// UUIDv7 id, appends a `create` op to `entity_op` (the single writer also
    /// journals the op and reprojects FTS), upserts the note spine + detail,
    /// projects the body into `block` rows, extracts `link` edges, and emits
    /// [`AppEvent::NoteSaved`] + [`AppEvent::NoteProjected`]. Returns the note.
    pub fn create_note(
        &self,
        title: impl Into<String>,
        doc_json: Option<String>,
    ) -> AppResult<Note> {
        let title = title.into();
        let note_id = Id::new();
        let now = self.now_ms();
        let doc_json = doc_json.unwrap_or_else(empty_doc);

        let mut doc =
            notes::Node::from_json(&doc_json).map_err(|e| AppError::Validation(e.to_string()))?;
        let mut gen = || BlockId::new(util::mint_block_id());
        let (blocks, links) = notes::validate_and_project(note_id, &mut doc, &mut gen)
            .map_err(|e| AppError::Validation(e.to_string()))?;
        let stamped = doc
            .to_json()
            .map_err(|e| AppError::Serialization(e.to_string()))?;

        let mut cols = Columns::new();
        cols.insert("doc_json".into(), Value::String(stamped.clone()));
        cols.insert(
            "content_hash".into(),
            Value::String(util::content_hash(&stamped)),
        );
        cols.insert(
            "word_count".into(),
            Value::Number(body_word_count(&blocks).into()),
        );
        cols.insert("is_pinned".into(), Value::Bool(false));

        let spine_title = normalize_title(Some(title));
        self.commit(&util::create_op(
            note_id,
            self.next_hlc(),
            "note",
            spine_title,
            None,
            now,
            Some((DetailTable::Note, cols)),
        ))?;

        self.write_blocks(note_id, &blocks)?;
        self.reconcile_projected_links(note_id, &links)?;

        self.emit(AppEvent::NoteSaved {
            note_id,
            version: now as u64,
            changed_block_ids: blocks.iter().map(|b| b.block_id.clone()).collect(),
        });
        self.emit(AppEvent::NoteProjected { note_id });

        self.get_note(&note_id.to_string())
    }

    /// M0: read a note back by id (spine + `doc_json`).
    pub fn get_note(&self, note_id: &str) -> AppResult<Note> {
        let id = parse_id(note_id)?;
        let row = self
            .read(|c| Ok(read_note(c, id)?))?
            .ok_or_else(|| AppError::NotFound(format!("note {note_id}")))?;
        Ok(Note {
            id: note_id.to_string(),
            title: row.title,
            doc_json: row.doc_json,
            version: row.updated_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }

    /// M0: every live note, most-recently-updated first.
    pub fn list_notes(&self) -> AppResult<Vec<Note>> {
        let summaries = self.notes_list(None)?;
        let mut out = Vec::with_capacity(summaries.len());
        for s in summaries {
            out.push(self.get_note(&s.id)?);
        }
        Ok(out)
    }

    /// M0: delta-update a note's `title` and/or `doc_json`. Absent fields are
    /// left unchanged. A body change re-projects blocks + links; the spine op
    /// bumps `updated_at`. Emits [`AppEvent::NoteSaved`] + [`AppEvent::NoteProjected`].
    pub fn update_note(
        &self,
        note_id: &str,
        title: Option<String>,
        doc_json: Option<String>,
    ) -> AppResult<Note> {
        let id = parse_id(note_id)?;
        let now = self.now_ms();
        let spine = self
            .read(|c| Ok(util::read_spine(c, id)?))?
            .ok_or_else(|| AppError::NotFound(format!("note {note_id}")))?;
        if spine.kind != "note" {
            return Err(AppError::Validation(format!("{note_id} is not a note")));
        }

        // Project the new body up front (if supplied) so the spine op and the
        // derived block/link ops commit as a coherent unit.
        let mut cols = Columns::new();
        let mut projected: Option<(Vec<notes::ProjectedBlock>, Vec<notes::ExtractedLink>)> = None;
        if let Some(doc_json) = doc_json {
            let mut doc = notes::Node::from_json(&doc_json)
                .map_err(|e| AppError::Validation(e.to_string()))?;
            let mut gen = || BlockId::new(util::mint_block_id());
            let (blocks, links) = notes::validate_and_project(id, &mut doc, &mut gen)
                .map_err(|e| AppError::Validation(e.to_string()))?;
            let stamped = doc
                .to_json()
                .map_err(|e| AppError::Serialization(e.to_string()))?;
            cols.insert("doc_json".into(), Value::String(stamped.clone()));
            cols.insert(
                "content_hash".into(),
                Value::String(util::content_hash(&stamped)),
            );
            cols.insert(
                "word_count".into(),
                Value::Number(body_word_count(&blocks).into()),
            );
            projected = Some((blocks, links));
        }

        let detail = if cols.is_empty() {
            None
        } else {
            Some((DetailTable::Note, cols))
        };
        self.commit(&util::update_op(
            id,
            self.next_hlc(),
            &spine,
            normalize_title(title),
            now,
            detail,
        ))?;

        let changed: Vec<BlockId> = if let Some((blocks, links)) = projected {
            self.write_blocks(id, &blocks)?;
            self.reconcile_projected_links(id, &links)?;
            blocks.iter().map(|b| b.block_id.clone()).collect()
        } else {
            Vec::new()
        };

        self.emit(AppEvent::NoteSaved {
            note_id: id,
            version: now as u64,
            changed_block_ids: changed,
        });
        self.emit(AppEvent::NoteProjected { note_id: id });

        self.get_note(note_id)
    }

    /// `notes.create` — seed a note, project its (possibly empty) body, emit events.
    pub fn notes_create(
        &self,
        notebook_id: Option<String>,
        daily_date: Option<String>,
        doc_json: Option<String>,
    ) -> AppResult<String> {
        let note_id = Id::new();
        let now = self.now_ms();
        let doc_json = doc_json.unwrap_or_else(empty_doc);

        // Build the initial detail columns.
        let mut doc =
            notes::Node::from_json(&doc_json).map_err(|e| AppError::Validation(e.to_string()))?;
        let mut gen = || BlockId::new(util::mint_block_id());
        let (blocks, links) = notes::validate_and_project(note_id, &mut doc, &mut gen)
            .map_err(|e| AppError::Validation(e.to_string()))?;
        let stamped = doc
            .to_json()
            .map_err(|e| AppError::Serialization(e.to_string()))?;
        let title = derive_title(&blocks);

        let mut cols = Columns::new();
        cols.insert("doc_json".into(), Value::String(stamped.clone()));
        cols.insert(
            "content_hash".into(),
            Value::String(util::content_hash(&stamped)),
        );
        cols.insert(
            "word_count".into(),
            Value::Number(body_word_count(&blocks).into()),
        );
        cols.insert("is_pinned".into(), Value::Bool(false));
        if let Some(nb) = &notebook_id {
            parse_id(nb)?; // validate
            cols.insert("notebook_id".into(), Value::String(nb.clone()));
        }
        if let Some(d) = &daily_date {
            cols.insert("daily_date".into(), Value::String(d.clone()));
        }

        self.commit(&util::create_op(
            note_id,
            self.next_hlc(),
            "note",
            title,
            daily_date,
            now,
            Some((DetailTable::Note, cols)),
        ))?;

        self.write_blocks(note_id, &blocks)?;
        self.reconcile_projected_links(note_id, &links)?;

        self.emit(AppEvent::NoteSaved {
            note_id,
            version: now as u64,
            changed_block_ids: blocks.iter().map(|b| b.block_id.clone()).collect(),
        });
        self.emit(AppEvent::NoteProjected { note_id });
        Ok(note_id.to_string())
    }

    /// `notes.get`.
    pub fn notes_get(&self, note_id: &str) -> AppResult<NoteView> {
        let id = parse_id(note_id)?;
        let row = self
            .read(|c| Ok(read_note(c, id)?))?
            .ok_or_else(|| AppError::NotFound(format!("note {note_id}")))?;
        Ok(NoteView {
            id: note_id.to_string(),
            title: row.title,
            doc_json: row.doc_json,
            notebook_id: row.notebook_id.map(hex_id),
            daily_date: row.daily_date,
            is_pinned: row.is_pinned,
            version: row.updated_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }

    /// `notes.save` — schema-validate, optimistic-concurrency check, project, and
    /// re-link (HLD §8.1). Content-hash gated: a body-unchanged save is a no-op.
    pub fn notes_save(
        &self,
        note_id: &str,
        doc_json: &str,
        base_version: i64,
    ) -> AppResult<SaveResult> {
        let id = parse_id(note_id)?;
        let now = self.now_ms();

        let (spine, current) = {
            let id2 = id;
            let spine = self
                .read(|c| Ok(util::read_spine(c, id2)?))?
                .ok_or_else(|| AppError::NotFound(format!("note {note_id}")))?;
            let row = self
                .read(|c| Ok(read_note(c, id2)?))?
                .ok_or_else(|| AppError::NotFound(format!("note {note_id}")))?;
            (spine, row)
        };
        if spine.kind != "note" {
            return Err(AppError::Validation(format!("{note_id} is not a note")));
        }
        // Optimistic concurrency: version == entity.updated_at (ms). base_version
        // == 0 is treated as "unversioned first write" and skips the check.
        if base_version != 0 && base_version != current.updated_at {
            return Err(AppError::Conflict(format!(
                "stale base_version {base_version} (current {})",
                current.updated_at
            )));
        }

        let mut doc =
            notes::Node::from_json(doc_json).map_err(|e| AppError::Validation(e.to_string()))?;
        let mut gen = || BlockId::new(util::mint_block_id());
        let (blocks, links) = notes::validate_and_project(id, &mut doc, &mut gen)
            .map_err(|e| AppError::Validation(e.to_string()))?;
        let stamped = doc
            .to_json()
            .map_err(|e| AppError::Serialization(e.to_string()))?;
        let new_hash = util::content_hash(&stamped);

        // Content-hash gate (HLD §8.1): no body change → no re-projection/re-embed.
        if current.content_hash.as_deref() == Some(new_hash.as_str()) {
            return Ok(SaveResult {
                version: current.updated_at,
                changed_block_ids: Vec::new(),
            });
        }

        let title = derive_title(&blocks);
        let mut cols = Columns::new();
        cols.insert("doc_json".into(), Value::String(stamped.clone()));
        cols.insert("content_hash".into(), Value::String(new_hash));
        cols.insert(
            "word_count".into(),
            Value::Number(body_word_count(&blocks).into()),
        );

        self.commit(&util::update_op(
            id,
            self.next_hlc(),
            &spine,
            title,
            now,
            Some((DetailTable::Note, cols)),
        ))?;

        self.write_blocks(id, &blocks)?;
        let resolved = self.reconcile_projected_links(id, &links)?;

        let changed: Vec<BlockId> = blocks.iter().map(|b| b.block_id.clone()).collect();
        self.emit(AppEvent::NoteSaved {
            note_id: id,
            version: now as u64,
            changed_block_ids: changed.clone(),
        });
        self.emit(AppEvent::NoteProjected { note_id: id });
        for target in resolved {
            let count = self
                .read(|c| Ok(backlink_count(c, target.id)?))
                .unwrap_or(0);
            self.emit(AppEvent::BacklinksChanged {
                target_ref: target,
                count,
            });
        }

        Ok(SaveResult {
            version: now,
            changed_block_ids: changed.iter().map(|b| b.0.clone()).collect(),
        })
    }

    /// `notes.list`.
    pub fn notes_list(&self, notebook_id: Option<String>) -> AppResult<Vec<NoteSummary>> {
        let filter_nb = match notebook_id {
            Some(nb) => Some(parse_id(&nb)?),
            None => None,
        };
        self.read(|c| {
            let mut out = Vec::new();
            let sql = "SELECT e.id, e.title, n.daily_date, n.is_pinned, e.updated_at \
                       FROM note n JOIN entity e ON e.id = n.entity_id \
                       WHERE e.deleted_at IS NULL \
                       ORDER BY e.updated_at DESC";
            let mut stmt = c.prepare(sql)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, Vec<u8>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            for (id, title, daily, pinned, updated) in rows {
                // notebook filter applied in-memory (schema stores it on `note`).
                if let Some(want) = filter_nb {
                    let has = note_notebook(c, Id::from_bytes(to16(&id)))?;
                    if has != Some(want) {
                        continue;
                    }
                }
                out.push(NoteSummary {
                    id: hex_id(id),
                    title,
                    daily_date: daily,
                    is_pinned: pinned != 0,
                    updated_at: updated,
                });
            }
            Ok(out)
        })
    }

    /// `notes.delete` — soft-delete tombstone (Data Model §3.1).
    pub fn notes_delete(&self, note_id: &str) -> AppResult<()> {
        let id = parse_id(note_id)?;
        let now = self.now_ms();
        self.commit(&util::delete_op(id, self.next_hlc(), now))?;
        self.emit(AppEvent::NoteProjected { note_id: id });
        Ok(())
    }

    /// `blocks.get`.
    pub fn blocks_get(&self, block_id: &str) -> AppResult<BlockView> {
        self.read(|c| {
            c.query_row(
                "SELECT note_id, block_id, node_type, seq, depth, text_content, order_key \
                 FROM block WHERE block_id = ?1 LIMIT 1",
                params![block_id],
                |r| {
                    Ok(BlockView {
                        note_id: hex_id(r.get::<_, Vec<u8>>(0)?),
                        block_id: r.get(1)?,
                        node_type: r.get(2)?,
                        seq: r.get(3)?,
                        depth: r.get(4)?,
                        text_content: r.get(5)?,
                        order_key: r.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
        })?
        .ok_or_else(|| AppError::NotFound(format!("block {block_id}")))
    }

    /// `blocks.backlinks` / linked references — a derived-on-read query over `link`.
    pub fn blocks_backlinks(&self, target: EntityRef) -> AppResult<Vec<BacklinkView>> {
        self.read(|c| {
            let rows = links::backlinks(c, target.id)
                .map_err(|e| storage::StorageError::Invariant(e.to_string()))?;
            let mut out = Vec::with_capacity(rows.len());
            for b in rows {
                // Enrich with the source entity's kind + title.
                let (kind, title): (Option<String>, Option<String>) = c
                    .query_row(
                        "SELECT kind, title FROM entity WHERE id = ?1",
                        params![b.src_entity.as_bytes().as_slice()],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?
                    .unwrap_or((None, None));
                out.push(BacklinkView {
                    src_entity: b.src_entity.to_string(),
                    src_kind: kind.unwrap_or_default(),
                    src_title: title,
                    src_block_id: b.src_block_id,
                    rel: b.rel.as_str().to_string(),
                });
            }
            Ok(out)
        })
    }

    /// `notes.resolveLinks` — wikilink target resolution incl. create-on-miss stubs.
    pub fn notes_resolve_links(&self, note_id: &str) -> AppResult<Vec<LinkResolution>> {
        let id = parse_id(note_id)?;
        let row = self
            .read(|c| Ok(read_note(c, id)?))?
            .ok_or_else(|| AppError::NotFound(format!("note {note_id}")))?;
        let doc = notes::Node::from_json(&row.doc_json)
            .map_err(|e| AppError::Validation(e.to_string()))?;
        let extracted = notes::extract_links(&doc);
        let mut out = Vec::new();
        for link in &extracted {
            let Some(title) = link.target_title.clone() else {
                continue;
            };
            let (dst, created) = self.resolve_target(link)?;
            out.push(LinkResolution {
                target_title: title,
                rel: link.rel.as_str().to_string(),
                resolved_id: dst.map(|d| d.to_string()),
                created_stub: created,
            });
        }
        Ok(out)
    }

    // -- internal write helpers ---------------------------------------------

    fn write_blocks(&self, note_id: Id, blocks: &[notes::ProjectedBlock]) -> AppResult<()> {
        for b in blocks {
            let op = storage::EntityOp::new(
                note_id,
                self.next_hlc(),
                OpBody::BlockSet {
                    block: BlockRow {
                        note_id,
                        block_id: b.block_id.0.clone(),
                        node_type: b.node_type.clone(),
                        seq: b.seq,
                        depth: b.depth,
                        text_content: Some(b.text_content.clone()),
                        attrs_json: b.attrs_json.clone(),
                        order_key: b.order_key.clone(),
                    },
                },
            );
            self.commit(&op)?;
        }
        Ok(())
    }

    /// Delete-and-reinsert the note's `projected` edges through the op-log so the
    /// graph rebuilds from the log (HLD §8.1). Returns the resolved target refs for
    /// `BacklinksChanged` emission.
    fn reconcile_projected_links(
        &self,
        note_id: Id,
        links_in: &[notes::ExtractedLink],
    ) -> AppResult<Vec<EntityRef>> {
        // 1. Tombstone existing projected edges for this note.
        let existing: Vec<Id> = self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id FROM link WHERE src_entity = ?1 AND origin = 'projected' \
                 AND deleted_at IS NULL",
            )?;
            let rows = stmt
                .query_map(params![note_id.as_bytes().as_slice()], |r| {
                    r.get::<_, Vec<u8>>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows.into_iter().map(|b| Id::from_bytes(to16(&b))).collect())
        })?;
        let now = self.now_ms();
        for lid in existing {
            self.commit(&storage::EntityOp::new(
                note_id,
                self.next_hlc(),
                OpBody::LinkDelete {
                    link_id: lid,
                    at: now,
                },
            ))?;
        }

        // 2. Resolve + insert the current edge set.
        let mut targets = Vec::new();
        for link in links_in {
            if link.target_title.is_none() && link.target_id.is_none() {
                continue;
            }
            let (dst, _created) = self.resolve_target(link)?;
            let Some(dst) = dst else { continue };
            let hlc = self.next_hlc();
            let row = LinkRow {
                id: Id::new(),
                src_entity: note_id,
                dst_entity: dst,
                rel: link.rel.as_str().to_string(),
                src_block_id: link.src_block_id.as_ref().map(|b| b.0.clone()),
                dst_block_id: link.target_block_id.as_ref().map(|b| b.0.clone()),
                evidence_segment_ids: None,
                data_json: if link.is_embed {
                    Some(r#"{"embed":true}"#.to_string())
                } else {
                    None
                },
                origin: "projected".to_string(),
                created_at: now,
                hlc: hlc.to_string(),
            };
            self.commit(&storage::EntityOp::new(
                note_id,
                hlc,
                OpBody::LinkSet { link: row },
            ))?;
            targets.push(EntityRef::new(kind_for_rel(link.rel), dst));
        }
        Ok(targets)
    }

    /// Resolve a wikilink/tag/mention target to a `dst` entity, creating a stub
    /// entity on miss. Returns `(dst_id, created_stub)`.
    fn resolve_target(&self, link: &notes::ExtractedLink) -> AppResult<(Option<Id>, bool)> {
        if let Some(id) = link.target_id {
            return Ok((Some(id), false));
        }
        let Some(title) = link.target_title.clone() else {
            return Ok((None, false));
        };
        let now = self.now_ms();
        match link.rel {
            LinkRel::Tagged => {
                let name = title.to_lowercase();
                if let Some(id) = self.read(|c| Ok(find_tag(c, &name)?))? {
                    return Ok((Some(id), false));
                }
                let id = Id::new();
                let mut cols = Columns::new();
                cols.insert("name".into(), Value::String(name));
                cols.insert("display".into(), Value::String(title.clone()));
                self.commit(&util::create_op(
                    id,
                    self.next_hlc(),
                    "tag",
                    Some(title),
                    None,
                    now,
                    Some((DetailTable::Tag, cols)),
                ))?;
                self.emit(AppEvent::TagChanged { tag_id: id });
                Ok((Some(id), true))
            }
            LinkRel::Wikilink => {
                if let Some(id) = self.read(|c| Ok(find_note_by_title(c, &title)?))? {
                    return Ok((Some(id), false));
                }
                // Create a stub note carrying the title (empty body).
                let id = Id::new();
                let mut cols = Columns::new();
                cols.insert("doc_json".into(), Value::String(empty_doc()));
                cols.insert(
                    "content_hash".into(),
                    Value::String(util::content_hash(&empty_doc())),
                );
                self.commit(&util::create_op(
                    id,
                    self.next_hlc(),
                    "note",
                    Some(title),
                    None,
                    now,
                    Some((DetailTable::Note, cols)),
                ))?;
                Ok((Some(id), true))
            }
            LinkRel::Mention => {
                let canonical = title.to_lowercase();
                if let Some(id) = self.read(|c| Ok(find_person(c, &canonical)?))? {
                    return Ok((Some(id), false));
                }
                let id = Id::new();
                let mut cols = Columns::new();
                cols.insert("display".into(), Value::String(title.clone()));
                cols.insert("canonical".into(), Value::String(canonical));
                self.commit(&util::create_op(
                    id,
                    self.next_hlc(),
                    "person",
                    Some(title),
                    None,
                    now,
                    Some((DetailTable::Person, cols)),
                ))?;
                Ok((Some(id), true))
            }
            _ => Ok((None, false)),
        }
    }
}

// ===========================================================================
// M1 use cases: daily notes, backlinks (with snippets), Markdown I/O
// ===========================================================================

impl Service {
    /// `daily.get_or_create` — the daily note keyed by `entity.daily_date =`
    /// `<local date>` (Feature Specs §1.4). Returns the existing note for `date`
    /// or, on miss, creates one titled by the date with `daily_date` set. The
    /// get path commits nothing (idempotent); only a miss appends ops.
    pub fn daily_get_or_create(&self, date: &str) -> AppResult<Note> {
        // Validate the local wall-date shape up front (`YYYY-MM-DD`).
        chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|_| AppError::Validation(format!("invalid daily date '{date}'")))?;

        if let Some(id) = self.read(|c| Ok(find_daily(c, date)?))? {
            return self.get_note(&id.to_string());
        }

        // Miss: seed an empty daily note carrying `daily_date` on spine + detail.
        let note_id = Id::new();
        let now = self.now_ms();
        let doc_json = empty_doc();
        let mut doc =
            notes::Node::from_json(&doc_json).map_err(|e| AppError::Validation(e.to_string()))?;
        let mut gen = || BlockId::new(util::mint_block_id());
        let (blocks, links) = notes::validate_and_project(note_id, &mut doc, &mut gen)
            .map_err(|e| AppError::Validation(e.to_string()))?;
        let stamped = doc
            .to_json()
            .map_err(|e| AppError::Serialization(e.to_string()))?;

        let mut cols = Columns::new();
        cols.insert("doc_json".into(), Value::String(stamped.clone()));
        cols.insert(
            "content_hash".into(),
            Value::String(util::content_hash(&stamped)),
        );
        cols.insert(
            "word_count".into(),
            Value::Number(body_word_count(&blocks).into()),
        );
        cols.insert("is_pinned".into(), Value::Bool(false));
        cols.insert("daily_date".into(), Value::String(date.to_string()));

        self.commit(&util::create_op(
            note_id,
            self.next_hlc(),
            "note",
            Some(date.to_string()),
            Some(date.to_string()),
            now,
            Some((DetailTable::Note, cols)),
        ))?;

        self.write_blocks(note_id, &blocks)?;
        self.reconcile_projected_links(note_id, &links)?;

        self.emit(AppEvent::NoteSaved {
            note_id,
            version: now as u64,
            changed_block_ids: blocks.iter().map(|b| b.block_id.clone()).collect(),
        });
        self.emit(AppEvent::NoteProjected { note_id });

        self.get_note(&note_id.to_string())
    }

    /// `links.backlinks` — the target's "Linked mentions" (resolved `[[wiki]]` /
    /// `@mention` edges pointing at it), each enriched with the source note title
    /// and a snippet of the referencing block (Feature Specs §1.2). Derived-on-read.
    pub fn links_backlinks(&self, entity_id: &str) -> AppResult<Vec<BacklinkRef>> {
        let id = parse_id(entity_id)?;
        self.read(|c| {
            let rows = links::backlinks(c, id)
                .map_err(|e| storage::StorageError::Invariant(e.to_string()))?;
            let mut out = Vec::with_capacity(rows.len());
            for b in rows {
                let title: Option<String> = c
                    .query_row(
                        "SELECT title FROM entity WHERE id = ?1",
                        params![b.src_entity.as_bytes().as_slice()],
                        |r| r.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten();
                let snippet = match &b.src_block_id {
                    Some(bid) => block_text(c, b.src_entity, bid)?.unwrap_or_default(),
                    None => String::new(),
                };
                out.push(BacklinkRef {
                    source_note_id: b.src_entity.to_string(),
                    source_title: title,
                    block_id: b.src_block_id.clone(),
                    snippet: truncate_snippet(&snippet, 160),
                });
            }
            Ok(out)
        })
    }

    /// `links.unlinked_mentions` — notes whose text matches the target's title via
    /// FTS but that carry no edge to it yet (Feature Specs §1.2). Surfaced live.
    pub fn links_unlinked_mentions(&self, entity_id: &str) -> AppResult<Vec<UnlinkedMention>> {
        let id = parse_id(entity_id)?;
        let title: Option<String> = self.read(|c| {
            Ok(c.query_row(
                "SELECT title FROM entity WHERE id = ?1 AND deleted_at IS NULL",
                params![id.as_bytes().as_slice()],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
        })?;
        let Some(title) = title else {
            return Ok(Vec::new());
        };
        if title.trim().is_empty() {
            return Ok(Vec::new());
        }
        let fts_match = fts_phrase(&title);
        self.read(move |c| {
            let ids = links::unlinked_mentions(c, id, &fts_match)
                .map_err(|e| storage::StorageError::Invariant(e.to_string()))?;
            let mut out = Vec::with_capacity(ids.len());
            for src in ids {
                let src_title: Option<String> = c
                    .query_row(
                        "SELECT title FROM entity WHERE id = ?1",
                        params![src.as_bytes().as_slice()],
                        |r| r.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten();
                let snippet = first_block_text(c, src)?
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| src_title.clone())
                    .unwrap_or_default();
                out.push(UnlinkedMention {
                    source_note_id: src.to_string(),
                    source_title: src_title,
                    snippet: truncate_snippet(&snippet, 160),
                });
            }
            Ok(out)
        })
    }

    /// `notes.export_markdown` — render a note's `doc_json` to CommonMark+GFM
    /// (Data Model §15.1, Feature Specs §8.1), preserving `[[wikilink]]`/`#tag`/
    /// `@mention` inline forms so a re-import round-trips.
    pub fn notes_export_markdown(&self, note_id: &str) -> AppResult<String> {
        let id = parse_id(note_id)?;
        let row = self
            .read(|c| Ok(read_note(c, id)?))?
            .ok_or_else(|| AppError::NotFound(format!("note {note_id}")))?;
        let doc = notes::Node::from_json(&row.doc_json)
            .map_err(|e| AppError::Validation(e.to_string()))?;
        Ok(notes::to_markdown(&doc))
    }

    /// `notes.import_markdown` — parse Markdown into a fresh note: mint blockIds,
    /// project blocks, extract + resolve links, optionally file it under
    /// `notebook_id`. Import never overwrites — it always creates a new entity
    /// (Feature Specs §8.1). Returns the created note.
    pub fn notes_import_markdown(&self, md: &str, notebook_id: Option<String>) -> AppResult<Note> {
        let note_id = Id::new();
        let now = self.now_ms();

        let mut doc = notes::from_markdown(md);
        let mut gen = || BlockId::new(util::mint_block_id());
        let (blocks, links) = notes::validate_and_project(note_id, &mut doc, &mut gen)
            .map_err(|e| AppError::Validation(e.to_string()))?;
        let stamped = doc
            .to_json()
            .map_err(|e| AppError::Serialization(e.to_string()))?;
        let title = derive_title(&blocks);

        let mut cols = Columns::new();
        cols.insert("doc_json".into(), Value::String(stamped.clone()));
        cols.insert(
            "content_hash".into(),
            Value::String(util::content_hash(&stamped)),
        );
        cols.insert(
            "word_count".into(),
            Value::Number(body_word_count(&blocks).into()),
        );
        cols.insert("is_pinned".into(), Value::Bool(false));
        if let Some(nb) = &notebook_id {
            let nbid = parse_id(nb)?;
            let nb_spine = self
                .read(|c| Ok(util::read_spine(c, nbid)?))?
                .ok_or_else(|| AppError::NotFound(format!("notebook {nb}")))?;
            if nb_spine.kind != "notebook" {
                return Err(AppError::Validation(format!("{nb} is not a notebook")));
            }
            cols.insert("notebook_id".into(), Value::String(nbid.to_string()));
        }

        self.commit(&util::create_op(
            note_id,
            self.next_hlc(),
            "note",
            title,
            None,
            now,
            Some((DetailTable::Note, cols)),
        ))?;

        self.write_blocks(note_id, &blocks)?;
        self.reconcile_projected_links(note_id, &links)?;

        self.emit(AppEvent::NoteSaved {
            note_id,
            version: now as u64,
            changed_block_ids: blocks.iter().map(|b| b.block_id.clone()).collect(),
        });
        self.emit(AppEvent::NoteProjected { note_id });

        self.get_note(&note_id.to_string())
    }
}

// ---------------------------------------------------------------------------
// Small read helpers + conversions
// ---------------------------------------------------------------------------

/// Find the live daily note for a local `date` (`YYYY-MM-DD`), if one exists.
fn find_daily(conn: &Connection, date: &str) -> rusqlite::Result<Option<Id>> {
    conn.query_row(
        "SELECT e.id FROM note n JOIN entity e ON e.id = n.entity_id \
         WHERE n.daily_date = ?1 AND e.deleted_at IS NULL LIMIT 1",
        params![date],
        |r| r.get::<_, Vec<u8>>(0),
    )
    .optional()
    .map(|o| o.map(|b| Id::from_bytes(to16(&b))))
}

/// The text of one projected block (for a backlink snippet).
fn block_text(conn: &Connection, note_id: Id, block_id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT text_content FROM block WHERE note_id = ?1 AND block_id = ?2",
        params![note_id.as_bytes().as_slice(), block_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()
    .map(|o| o.flatten())
}

/// The first block's text of a note (for an unlinked-mention snippet).
fn first_block_text(conn: &Connection, note_id: Id) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT text_content FROM block WHERE note_id = ?1 ORDER BY seq LIMIT 1",
        params![note_id.as_bytes().as_slice()],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()
    .map(|o| o.flatten())
}

/// Quote a title as an FTS5 phrase (embedded `"` doubled), so multi-word titles
/// match as a phrase rather than as independent OR terms.
fn fts_phrase(title: &str) -> String {
    format!("\"{}\"", title.replace('"', "\"\""))
}

/// Trim + cap a snippet to `max` characters, appending an ellipsis on truncation.
fn truncate_snippet(text: &str, max: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= max {
        t.to_string()
    } else {
        let head: String = t.chars().take(max).collect();
        format!("{head}...")
    }
}

fn find_tag(conn: &Connection, name: &str) -> rusqlite::Result<Option<Id>> {
    conn.query_row(
        "SELECT e.id FROM entity e JOIN tag t ON t.entity_id = e.id \
         WHERE t.name = ?1 AND e.deleted_at IS NULL LIMIT 1",
        params![name],
        |r| r.get::<_, Vec<u8>>(0),
    )
    .optional()
    .map(|o| o.map(|b| Id::from_bytes(to16(&b))))
}

fn find_note_by_title(conn: &Connection, title: &str) -> rusqlite::Result<Option<Id>> {
    conn.query_row(
        "SELECT id FROM entity WHERE kind = 'note' AND title = ?1 COLLATE NOCASE \
         AND deleted_at IS NULL LIMIT 1",
        params![title],
        |r| r.get::<_, Vec<u8>>(0),
    )
    .optional()
    .map(|o| o.map(|b| Id::from_bytes(to16(&b))))
}

fn find_person(conn: &Connection, canonical: &str) -> rusqlite::Result<Option<Id>> {
    conn.query_row(
        "SELECT e.id FROM entity e JOIN person p ON p.entity_id = e.id \
         WHERE p.canonical = ?1 AND e.deleted_at IS NULL LIMIT 1",
        params![canonical],
        |r| r.get::<_, Vec<u8>>(0),
    )
    .optional()
    .map(|o| o.map(|b| Id::from_bytes(to16(&b))))
}

fn note_notebook(conn: &Connection, id: Id) -> rusqlite::Result<Option<Id>> {
    conn.query_row(
        "SELECT notebook_id FROM note WHERE entity_id = ?1",
        params![id.as_bytes().as_slice()],
        |r| r.get::<_, Option<Vec<u8>>>(0),
    )
    .optional()
    .map(|o| o.flatten().map(|b| Id::from_bytes(to16(&b))))
}

fn backlink_count(conn: &Connection, target: Id) -> rusqlite::Result<u32> {
    conn.query_row(
        "SELECT count(*) FROM link WHERE dst_entity = ?1 AND deleted_at IS NULL",
        params![target.as_bytes().as_slice()],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as u32)
}

/// Trim a caller-supplied title, collapsing an empty/whitespace string to `None`
/// so the spine `title` stays `NULL` rather than an empty string.
fn normalize_title(title: Option<String>) -> Option<String> {
    title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn derive_title(blocks: &[notes::ProjectedBlock]) -> Option<String> {
    blocks
        .iter()
        .map(|b| b.text_content.trim())
        .find(|t| !t.is_empty())
        .map(|t| t.chars().take(120).collect())
}

fn body_word_count(blocks: &[notes::ProjectedBlock]) -> i64 {
    let joined: String = blocks
        .iter()
        .map(|b| b.text_content.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    util::word_count(&joined)
}

fn kind_for_rel(rel: LinkRel) -> EntityKind {
    match rel {
        LinkRel::Tagged => EntityKind::Tag,
        LinkRel::Mention => EntityKind::Person,
        _ => EntityKind::Note,
    }
}

/// Lower-hex of a 16-byte id blob → hyphenated UUID string.
fn hex_id(b: Vec<u8>) -> String {
    Id::from_bytes(to16(&b)).to_string()
}

fn to16(b: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let n = b.len().min(16);
    out[..n].copy_from_slice(&b[..n]);
    out
}
