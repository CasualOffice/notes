//! Neighborhood-graph query skeleton (Data Model §5, HLD §4 "graph" surface).
//!
//! A breadth-first expansion from a center entity over the live `link` graph, in
//! **both** directions, out to a bounded depth. Feeds the graph view; the layout
//! itself is a UI concern. This is a skeleton — ranking, type filters, and edge
//! weighting land with the search/graph feature work.

use std::collections::HashSet;

use app_domain::{Id, LinkRel};
use rusqlite::{params, Connection};

use crate::edge::{id_from_blob, rel_from_str};
use crate::error::Result;

/// A directed edge in the neighborhood result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GraphEdge {
    /// Source entity.
    pub src: Id,
    /// Destination entity.
    pub dst: Id,
    /// Relationship.
    pub rel: LinkRel,
}

/// The result of a [`neighborhood`] expansion.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NeighborhoodGraph {
    /// The entity the expansion started from.
    pub center: Option<Id>,
    /// All reached entities (including the center), de-duplicated.
    pub nodes: Vec<Id>,
    /// All traversed edges, de-duplicated.
    pub edges: Vec<GraphEdge>,
}

/// Expand the live link graph around `center` up to `max_depth` hops in both
/// directions. `max_depth == 0` returns just the center node.
///
/// # Errors
/// Propagates any SQLite failure or a malformed stored `rel`/id.
pub fn neighborhood(conn: &Connection, center: Id, max_depth: u32) -> Result<NeighborhoodGraph> {
    let mut seen: HashSet<Id> = HashSet::new();
    let mut edge_set: HashSet<GraphEdge> = HashSet::new();
    let mut nodes: Vec<Id> = Vec::new();
    let mut edges: Vec<GraphEdge> = Vec::new();

    seen.insert(center);
    nodes.push(center);
    let mut frontier = vec![center];

    for _ in 0..max_depth {
        if frontier.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for node in frontier.drain(..) {
            for (neighbor, edge) in neighbors_of(conn, node)? {
                if edge_set.insert(edge) {
                    edges.push(edge);
                }
                if seen.insert(neighbor) {
                    nodes.push(neighbor);
                    next.push(neighbor);
                }
            }
        }
        frontier = next;
    }

    Ok(NeighborhoodGraph {
        center: Some(center),
        nodes,
        edges,
    })
}

/// One hop out of `node`, both outgoing and incoming, as `(neighbor, edge)` pairs.
fn neighbors_of(conn: &Connection, node: Id) -> Result<Vec<(Id, GraphEdge)>> {
    let blob = &node.as_bytes()[..];
    let mut out = Vec::new();

    // Outgoing: node -> other.
    let mut outgoing = conn.prepare_cached(
        "SELECT dst_entity, rel FROM link WHERE src_entity = ?1 AND deleted_at IS NULL",
    )?;
    let rows = outgoing.query_map(params![blob], |row| {
        Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (other, rel) = row?;
        let other = id_from_blob(&other)?;
        out.push((
            other,
            GraphEdge {
                src: node,
                dst: other,
                rel: rel_from_str(&rel)?,
            },
        ));
    }

    // Incoming: other -> node.
    let mut incoming = conn.prepare_cached(
        "SELECT src_entity, rel FROM link WHERE dst_entity = ?1 AND deleted_at IS NULL",
    )?;
    let rows = incoming.query_map(params![blob], |row| {
        Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (other, rel) = row?;
        let other = id_from_blob(&other)?;
        out.push((
            other,
            GraphEdge {
                src: other,
                dst: node,
                rel: rel_from_str(&rel)?,
            },
        ));
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::NewLink;
    use crate::reconcile::upsert_edge;
    use crate::test_support::fresh_db;

    #[test]
    fn depth_bounded_bidirectional_expansion() {
        let conn = fresh_db();
        let (a, b, c, d) = (Id::new(), Id::new(), Id::new(), Id::new());
        // a -> b -> c , and d -> a (incoming to the center).
        upsert_edge(&conn, &NewLink::new(a, b, LinkRel::Wikilink), 1, "h").unwrap();
        upsert_edge(&conn, &NewLink::new(b, c, LinkRel::Wikilink), 2, "h").unwrap();
        upsert_edge(&conn, &NewLink::new(d, a, LinkRel::Mention), 3, "h").unwrap();

        // Depth 1 from a reaches b (out) and d (in) but not c.
        let g1 = neighborhood(&conn, a, 1).unwrap();
        assert!(g1.nodes.contains(&a) && g1.nodes.contains(&b) && g1.nodes.contains(&d));
        assert!(!g1.nodes.contains(&c));
        assert_eq!(g1.edges.len(), 2);

        // Depth 2 pulls c in via b.
        let g2 = neighborhood(&conn, a, 2).unwrap();
        assert!(g2.nodes.contains(&c));
        assert_eq!(g2.nodes.len(), 4);

        // Depth 0 is just the center.
        let g0 = neighborhood(&conn, a, 0).unwrap();
        assert_eq!(g0.nodes, vec![a]);
        assert!(g0.edges.is_empty());
    }
}
