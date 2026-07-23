//! The entity-op NDJSON journal — crash-safe write-ahead for the notebook
//! pillars. Implements Data Model §11.2 (the `journals/ops/<YYYY-MM-DD>.ndjson`
//! family) and the CLAUDE.md crash-safety invariant: *a `kill -9` must lose no
//! committed op; recovery replays the journal.*
//!
//! Each op is appended as one JSON line and `fsync`'d **before** its DB
//! transaction commits, so on recovery any op present in the journal but missing
//! from `entity_op` is re-applied.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use app_domain::{Hlc, Id, OpId, Timestamp};

use crate::error::{StorageError, StorageResult};
use crate::oplog::{EntityOp, OpBody, OpKind};

/// One NDJSON op record. Shape follows Data Model §11.2:
/// `{"op_id","entity","kind","hlc","actor","payload":{...},"t":<ms>}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpRecord {
    pub op_id: String,
    pub entity: String,
    pub kind: String,
    pub hlc: String,
    #[serde(default = "default_actor")]
    pub actor: String,
    pub payload: serde_json::Value,
    /// Wall-clock of the op, epoch-ms UTC (`created_at`).
    pub t: i64,
}

fn default_actor() -> String {
    "local".to_string()
}

impl OpRecord {
    /// Serialize an [`EntityOp`] into its journal record.
    pub fn from_op(op: &EntityOp) -> StorageResult<Self> {
        Ok(Self {
            op_id: op.op_id.to_string(),
            entity: op.entity_id.to_string(),
            kind: op.kind.as_str().to_string(),
            hlc: op.hlc.to_string(),
            actor: op.actor.clone(),
            payload: serde_json::to_value(&op.body)?,
            t: op.created_at.as_millis(),
        })
    }

    /// Reconstruct the typed [`EntityOp`] from this record (recovery replay).
    pub fn into_entity_op(self) -> StorageResult<EntityOp> {
        let body: OpBody = serde_json::from_value(self.payload)?;
        Ok(EntityOp {
            op_id: self
                .op_id
                .parse::<OpId>()
                .map_err(|_| StorageError::Invariant("journal: bad op_id".into()))?,
            entity_id: self
                .entity
                .parse::<Id>()
                .map_err(|_| StorageError::Invariant("journal: bad entity id".into()))?,
            kind: OpKind::from_db_str(&self.kind)
                .ok_or_else(|| StorageError::Invariant("journal: bad kind".into()))?,
            hlc: self
                .hlc
                .parse::<Hlc>()
                .map_err(|_| StorageError::Invariant("journal: bad hlc".into()))?,
            actor: self.actor,
            body,
            created_at: Timestamp::from_millis(self.t),
        })
    }
}

/// A daily-rotated, append-only op journal rooted at a directory.
#[derive(Clone, Debug)]
pub struct OpJournal {
    dir: PathBuf,
}

impl OpJournal {
    /// Journal writing into `dir` (typically `journals/ops/`). Creates the dir.
    pub fn new(dir: impl Into<PathBuf>) -> StorageResult<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// The file an op with wall-time `t_ms` rotates into (`<YYYY-MM-DD>.ndjson`).
    #[must_use]
    pub fn file_for(&self, t_ms: i64) -> PathBuf {
        let date = DateTime::<Utc>::from_timestamp_millis(t_ms)
            .unwrap_or_else(Utc::now)
            .format("%Y-%m-%d");
        self.dir.join(format!("{date}.ndjson"))
    }

    /// Append one record and `fsync` it durably before returning.
    pub fn append(&self, rec: &OpRecord) -> StorageResult<()> {
        let path = self.file_for(rec.t);
        let mut line = serde_json::to_string(rec)?;
        line.push('\n');
        let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
        f.write_all(line.as_bytes())?;
        f.flush()?;
        f.sync_all()?; // durability: the op is on disk before the DB commit
        Ok(())
    }

    /// Read every record across all rotated files, in (filename, line) order.
    pub fn replay(&self) -> StorageResult<Vec<OpRecord>> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "ndjson").unwrap_or(false))
            .collect();
        files.sort();

        let mut out = Vec::new();
        for path in files {
            out.extend(read_records(&path)?);
        }
        Ok(out)
    }
}

fn read_records(path: &Path) -> StorageResult<Vec<OpRecord>> {
    let f = File::open(path)?;
    let reader = BufReader::new(f);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(&line)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::oplog::{DetailFields, DetailTable, EntityOp, OpBody, SpineFields};

    fn temp_dir() -> PathBuf {
        let mut b = [0u8; 8];
        getrandom::getrandom(&mut b).unwrap();
        let name: String = b.iter().map(|x| format!("{x:02x}")).collect();
        std::env::temp_dir().join(format!("cn-journal-{name}"))
    }

    fn sample_op() -> EntityOp {
        let id = Id::new();
        let mut columns = BTreeMap::new();
        columns.insert(
            "order_key".to_string(),
            serde_json::Value::String("a0".into()),
        );
        EntityOp::new(
            id,
            Hlc::new(1000, 0, "nodeA"),
            OpBody::EntitySet {
                spine: SpineFields {
                    kind: "area".into(),
                    title: Some("Home".into()),
                    daily_date: None,
                    created_at: 1000,
                    updated_at: 1000,
                    deleted_at: None,
                },
                detail: Some(DetailFields {
                    table: DetailTable::Area,
                    columns,
                }),
            },
        )
    }

    #[test]
    fn append_then_replay_roundtrips() {
        let dir = temp_dir();
        let journal = OpJournal::new(&dir).unwrap();

        let op = sample_op();
        let rec = OpRecord::from_op(&op).unwrap();
        journal.append(&rec).unwrap();
        journal.append(&rec).unwrap();

        let back = journal.replay().unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].op_id, op.op_id.to_string());

        // Record → EntityOp → same op id / kind.
        let reop = back.into_iter().next().unwrap().into_entity_op().unwrap();
        assert_eq!(reop.op_id, op.op_id);
        assert_eq!(reop.entity_id, op.entity_id);

        std::fs::remove_dir_all(&dir).ok();
    }
}
