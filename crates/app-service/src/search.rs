//! Search + command palette (HLD §6/§8.5, Data Model §10). The `search` crate
//! builds parameterized FTS5 SQL; `app-service` executes it against the single
//! `storage` connection (the WebView never sees SQL) and assembles ranked hits.
//! Vector KNN + RRF re-fusion is the documented Phase-3 seam.

use app_domain::{AppError, AppResult, Day, Id};
use rusqlite::types::Value as SqlValue;
use rusqlite::{params_from_iter, Connection};
use search::{SearchMode, SearchQuery, SqlParam};

use crate::dto::{SearchHitDto, SearchResultsDto};
use crate::Service;

fn today(service: &Service) -> AppResult<Day> {
    let _ = service;
    crate::util::today_local()
        .parse::<Day>()
        .map_err(|e| AppError::Internal(format!("bad local date: {e}")))
}

fn to_sql(p: &SqlParam) -> SqlValue {
    match p {
        SqlParam::Text(s) => SqlValue::Text(s.clone()),
        SqlParam::Int(i) => SqlValue::Integer(*i),
    }
}

impl Service {
    /// `search.query` — synchronous FTS5(BM25) retrieval across the active sources.
    pub fn search_query(
        &self,
        q: &str,
        mode: &str,
        limit: Option<u32>,
    ) -> AppResult<SearchResultsDto> {
        let query_id = Id::new();
        let search_mode = match mode {
            "ask" => SearchMode::Ask,
            _ => SearchMode::Go,
        };
        let sq = SearchQuery::parse(q, search_mode, today(self)?);

        let hits = if sq.is_recents() {
            self.recents(limit.unwrap_or(20))?
        } else {
            self.run_fts(&sq)?
        };

        self.emit(app_domain::AppEvent::SearchPartial {
            query_id,
            hits: hits.len() as u32,
            source: app_domain::SearchSource::Fts,
        });

        Ok(SearchResultsDto {
            query_id: query_id.to_string(),
            hits,
            complete: true, // FTS-only Phase-1 result is complete (no vector channel yet)
        })
    }

    /// `palette.run` — Go (quick switch) / Do (command runner) / Ask (AI, stubbed).
    pub fn palette_run(&self, mode: &str, input: &str) -> AppResult<serde_json::Value> {
        match mode {
            "Go" | "go" => {
                let res = self.search_query(input, "go", Some(20))?;
                Ok(serde_json::to_value(res)?)
            }
            "Do" | "do" => {
                let cmds = search::match_commands(input);
                Ok(serde_json::to_value(cmds)?)
            }
            "Ask" | "ask" => Err(AppError::Internal(
                "palette Ask (grounded AI) is not yet implemented in this phase".into(),
            )),
            other => Err(AppError::Validation(format!(
                "unknown palette mode {other}"
            ))),
        }
    }

    fn run_fts(&self, sq: &SearchQuery) -> AppResult<Vec<SearchHitDto>> {
        let compiled = sq.compile();
        let text = sq.text.clone();
        self.read(move |c| {
            let mut hits: Vec<SearchHitDto> = Vec::new();
            for (source, cs) in &compiled {
                let mut stmt = c.prepare(&cs.sql)?;
                let binds: Vec<SqlValue> = cs.params.iter().map(to_sql).collect();
                let rows = stmt.query_map(params_from_iter(binds.iter()), |r| {
                    let entity_id: String = r.get(0)?;
                    let kind: String = r.get(1)?;
                    let title: Option<String> = r.get(2)?;
                    let rank: f64 = r.get(3)?;
                    Ok((entity_id, kind, title, rank))
                })?;
                for row in rows {
                    let (id_hex, kind, title, rank) = row?;
                    let Some(entity) = search::entity_ref_from_hex(&kind, &id_hex) else {
                        continue;
                    };
                    let snippet_src = title.clone().unwrap_or_default();
                    let snippet = search::make_snippet(&snippet_src, &text, 160);
                    let _ = source;
                    hits.push(SearchHitDto {
                        kind: entity.kind.as_str().to_string(),
                        id: entity.id.to_string(),
                        title,
                        snippet,
                        bm25: rank,
                    });
                }
            }
            // Lower bm25 = better (Data Model §10.1).
            hits.sort_by(|a, b| {
                a.bm25
                    .partial_cmp(&b.bm25)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            Ok(hits)
        })
    }

    /// Empty-query recents: most-recently-updated notes and tasks (Feature Specs §7.2).
    fn recents(&self, limit: u32) -> AppResult<Vec<SearchHitDto>> {
        self.read(move |c: &Connection| {
            let mut stmt = c.prepare(
                "SELECT lower(hex(id)) AS eid, kind, title FROM entity \
                 WHERE kind IN ('note','task') AND deleted_at IS NULL \
                 ORDER BY updated_at DESC LIMIT ?1",
            )?;
            let rows = stmt
                .query_map([limit as i64], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut hits = Vec::new();
            for (id_hex, kind, title) in rows {
                if let Some(entity) = search::entity_ref_from_hex(&kind, &id_hex) {
                    hits.push(SearchHitDto {
                        kind: entity.kind.as_str().to_string(),
                        id: entity.id.to_string(),
                        title,
                        snippet: String::new(),
                        bm25: 0.0,
                    });
                }
            }
            Ok(hits)
        })
    }
}
