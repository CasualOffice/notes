//! # links
//!
//! The one polymorphic link graph. Implements **Data Model §5** (`link`). All edges
//! — wikilinks, mentions, tags, meeting provenance, reminder targets, parentage —
//! live in one table. **Backlinks are a read, never a write** (§5.1): rendering
//! reverse references is `SELECT ... WHERE dst_entity = X`, never a materialized row.
//!
//! `origin='projected'` edges (parsed from `doc_json`) are rebuilt on save;
//! `user`/`meeting`/`ai_suggested` edges are never touched by projection. Tags/links
//! use OR-Set semantics for the dormant sync seam (HLD §10).
//!
//! ## Modules
//! - [`edge`] — the polymorphic edge model ([`NewLink`], [`LinkEdge`],
//!   [`LinkOrigin`]) mirroring the `link` columns.
//! - [`reconcile`] — OR-Set [`upsert_edge`] / [`tombstone_edge`] and the
//!   [`reconcile_projected`] delete-and-reinsert save path.
//! - [`query`] — the derived-on-read [`backlinks`] and [`unlinked_mentions`]
//!   builders.
//! - [`graph`] — the bidirectional, depth-bounded [`neighborhood`] expansion.
//!
//! All functions take a borrowed [`rusqlite::Connection`]; this crate opens no
//! connection and holds no key (the WebView never sees SQL — Architecture §12).

#![forbid(unsafe_code)]

pub mod edge;
pub mod error;
pub mod graph;
pub mod query;
pub mod reconcile;

#[cfg(test)]
pub(crate) mod test_support;

pub use edge::{id_from_blob, rel_from_str, LinkEdge, LinkOrigin, NewLink};
pub use error::{LinkError, Result};
pub use graph::{neighborhood, GraphEdge, NeighborhoodGraph};
pub use query::{backlinks, unlinked_mentions, Backlink, BACKLINKS_SQL, UNLINKED_MENTIONS_SQL};
pub use reconcile::{reconcile_projected, tombstone_edge, upsert_edge, ReconcileStats};
