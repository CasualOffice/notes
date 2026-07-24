//! CalDAV protocol logic — RFC 4791 (calendaring) + RFC 6578 (sync-collection).
//!
//! See `docs/casual-note-calendar.md` §3 (Tier B, the universal two-way path).
//! Everything here is written against the async [`Transport`] seam and the
//! `quick-xml` pull parser, so it opens **no socket of its own** and is fully
//! unit-testable with [`MockTransport`](crate::transport::MockTransport). The
//! production transport (the one place a socket may open — to the user's own
//! server, with credentials injected from the OS keystore) is supplied by the
//! app-service.
//!
//! Implemented requests:
//! - `PROPFIND` (Depth 1) — enumerate collections + `ctag`/`sync-token`/color.
//! - `REPORT calendar-query` — full pull of all `VEVENT`s.
//! - `REPORT sync-collection` (RFC 6578) — incremental pull via `sync-token`.
//! - `PUT` with `If-Match` / `If-None-Match` — create/update with ETag concurrency.
//! - `DELETE` with `If-Match` — remove with ETag concurrency.

use quick_xml::events::Event;
use quick_xml::name::QName;
use quick_xml::reader::Reader;
use quick_xml::XmlVersion;

use app_domain::Id;

use crate::error::{SyncError, SyncResult};
use crate::ical::{parse_ics, write_ics};
use crate::sync::{
    CalId, CalendarCapability, CalendarSyncAdapter, ChangeSet, EventOp, PushOutcome, PushResult,
    RemoteCalendar, RemoteEvent, SyncToken,
};
use crate::transport::{HttpRequest, Transport};

/// XML namespace prefixes bound in every request body we emit.
const XMLNS: &str = concat!(
    r#"xmlns:d="DAV:" "#,
    r#"xmlns:c="urn:ietf:params:xml:ns:caldav" "#,
    r#"xmlns:cs="http://calendarserver.org/ns/" "#,
    r#"xmlns:ic="http://apple.com/ns/ical/""#
);

/// A CalDAV client bound to one account's calendar-home-set URL, generic over the
/// [`Transport`] that actually performs I/O. Holds **no credentials** — auth is
/// the transport's concern (doc §7).
#[derive(Debug)]
pub struct CalDavClient<T: Transport> {
    transport: T,
    /// The calendar-home-set URL used by [`list_calendars`](Self::list_calendars).
    home_set_url: String,
}

impl<T: Transport> CalDavClient<T> {
    /// Build a client for a discovered calendar-home-set URL.
    #[must_use]
    pub fn new(transport: T, home_set_url: impl Into<String>) -> Self {
        Self {
            transport,
            home_set_url: home_set_url.into(),
        }
    }

    /// Borrow the underlying transport (useful for asserting on a mock in tests).
    #[must_use]
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// `PROPFIND` Depth 1 the home set and return the calendar collections found.
    pub async fn propfind_calendars(&self) -> SyncResult<Vec<RemoteCalendar>> {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind {XMLNS}>
  <d:prop>
    <d:resourcetype/>
    <d:displayname/>
    <cs:getctag/>
    <d:sync-token/>
    <ic:calendar-color/>
  </d:prop>
</d:propfind>"#
        );
        let req = HttpRequest::new("PROPFIND", &self.home_set_url)
            .header("Depth", "1")
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body);
        let resp = self.transport.request(req).await?;
        expect_multistatus(&resp)?;
        let ms = parse_multistatus(&resp.body)?;

        let mut calendars = Vec::new();
        for r in ms.responses {
            if !r.props.is_calendar {
                continue;
            }
            // Skip the home-set collection itself.
            if hrefs_equal(&r.href, &self.home_set_url) {
                continue;
            }
            calendars.push(RemoteCalendar {
                id: CalId(resolve_href(&self.home_set_url, &r.href)),
                name: r.props.displayname.unwrap_or_else(|| r.href.clone()),
                color: r.props.color,
                writable: true,
                ctag: r.props.getctag,
                sync_token: r
                    .props
                    .sync_token
                    .map_or_else(SyncToken::initial, SyncToken::some),
                tz: None,
            });
        }
        Ok(calendars)
    }

    /// Full pull: `REPORT calendar-query` for every `VEVENT` in the collection.
    pub async fn calendar_query(&self, cal_url: &str) -> SyncResult<ChangeSet> {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-query {XMLNS}>
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT"/>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#
        );
        let req = HttpRequest::new("REPORT", cal_url)
            .header("Depth", "1")
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body);
        let resp = self.transport.request(req).await?;
        expect_multistatus(&resp)?;
        let ms = parse_multistatus(&resp.body)?;
        self.changeset_from(cal_url, ms)
    }

    /// Incremental pull: `REPORT sync-collection` (RFC 6578). An empty `since`
    /// requests the full initial set; the returned [`ChangeSet::next_token`] is
    /// persisted and passed back next time. Idempotent.
    pub async fn sync_collection(&self, cal_url: &str, since: &SyncToken) -> SyncResult<ChangeSet> {
        let token = since.as_deref().unwrap_or("");
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:sync-collection {XMLNS}>
  <d:sync-token>{token}</d:sync-token>
  <d:sync-level>1</d:sync-level>
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
</d:sync-collection>"#
        );
        let req = HttpRequest::new("REPORT", cal_url)
            .header("Depth", "1")
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body);
        let resp = self.transport.request(req).await?;
        expect_multistatus(&resp)?;
        let ms = parse_multistatus(&resp.body)?;
        self.changeset_from(cal_url, ms)
    }

    /// Turn a parsed `Multi-Status` into a [`ChangeSet`]: 2xx responses with
    /// `calendar-data` become `changed`; 404 responses become `deleted` hrefs.
    fn changeset_from(&self, cal_url: &str, ms: MultiStatus) -> SyncResult<ChangeSet> {
        let mut changed = Vec::new();
        let mut deleted = Vec::new();
        for r in ms.responses {
            let href = resolve_href(cal_url, &r.href);
            if r.status == Some(404) || r.props.propstat_status == Some(404) {
                deleted.push(href);
                continue;
            }
            if let Some(data) = r.props.calendar_data {
                // Parse the inline VCALENDAR; assign a placeholder local calendar
                // id (the app-service rebinds it on ingest — reconciliation keys
                // by UID, not by this id).
                let mut events = parse_ics(&data, Id::new())?;
                let Some(mut ev) = events.drain(..).next() else {
                    continue;
                };
                ev.etag = r.props.getetag;
                changed.push(RemoteEvent { href, event: ev });
            }
        }
        Ok(ChangeSet {
            changed,
            deleted,
            next_token: ms
                .sync_token
                .map_or_else(SyncToken::initial, SyncToken::some),
        })
    }

    /// Push one batch of ops to a writable collection, returning per-op results.
    /// A 412 precondition failure is reported as [`PushOutcome::Conflict`] rather
    /// than an error — the conflict is *detected*, never silently lost (doc §3.2).
    pub async fn push_ops(&self, cal_url: &str, ops: &[EventOp]) -> SyncResult<Vec<PushResult>> {
        let mut results = Vec::with_capacity(ops.len());
        for op in ops {
            let result = match op {
                EventOp::Create(event) => {
                    let href =
                        resolve_href(cal_url, &format!("{}.ics", encode_segment(&event.uid)));
                    let body = write_ics(std::slice::from_ref(event))?;
                    let req = HttpRequest::new("PUT", &href)
                        .header("Content-Type", "text/calendar; charset=utf-8")
                        .header("If-None-Match", "*")
                        .body(body);
                    let resp = self.transport.request(req).await?;
                    interpret_put(&event.uid, href, None, &resp)?
                }
                EventOp::Update { event, href, etag } => {
                    let full = resolve_href(cal_url, href);
                    let body = write_ics(std::slice::from_ref(event))?;
                    let mut req = HttpRequest::new("PUT", &full)
                        .header("Content-Type", "text/calendar; charset=utf-8");
                    if let Some(tag) = etag {
                        req = req.header("If-Match", tag.clone());
                    }
                    let resp = self.transport.request(req.body(body)).await?;
                    interpret_put(&event.uid, full, etag.clone(), &resp)?
                }
                EventOp::Delete { uid, href, etag } => {
                    let full = resolve_href(cal_url, href);
                    let mut req = HttpRequest::new("DELETE", &full);
                    if let Some(tag) = etag {
                        req = req.header("If-Match", tag.clone());
                    }
                    let resp = self.transport.request(req).await?;
                    interpret_delete(uid, etag.clone(), &resp)?
                }
            };
            results.push(result);
        }
        Ok(results)
    }
}

impl<T: Transport> CalendarSyncAdapter for CalDavClient<T> {
    fn capability(&self) -> CalendarCapability {
        // Tier B is the universal full two-way path (doc §3).
        CalendarCapability::CalDav {
            read: true,
            write: true,
        }
    }

    async fn list_calendars(&self) -> SyncResult<Vec<RemoteCalendar>> {
        self.propfind_calendars().await
    }

    async fn pull(&self, cal: &CalId, since: &SyncToken) -> SyncResult<ChangeSet> {
        self.sync_collection(cal.as_str(), since).await
    }

    async fn push(&self, cal: &CalId, ops: &[EventOp]) -> SyncResult<Vec<PushResult>> {
        self.push_ops(cal.as_str(), ops).await
    }
}

/// Interpret a `PUT` response into a [`PushResult`]. `sent_etag` is the `If-Match`
/// value we submitted (echoed back in a conflict for the caller's audit).
fn interpret_put(
    uid: &str,
    href: String,
    sent_etag: Option<String>,
    resp: &crate::transport::HttpResponse,
) -> SyncResult<PushResult> {
    let outcome = match resp.status {
        200 | 201 | 204 => PushOutcome::Written {
            href,
            etag: resp.header_value("ETag").map(str::to_string),
        },
        412 => PushOutcome::Conflict {
            local_etag: sent_etag,
        },
        other => {
            return Err(SyncError::Http {
                status: other,
                message: format!("unexpected PUT status for uid {uid}"),
            })
        }
    };
    Ok(PushResult {
        uid: uid.to_string(),
        outcome,
    })
}

/// Interpret a `DELETE` response into a [`PushResult`].
fn interpret_delete(
    uid: &str,
    local_etag: Option<String>,
    resp: &crate::transport::HttpResponse,
) -> SyncResult<PushResult> {
    let outcome = match resp.status {
        200 | 202 | 204 | 404 => PushOutcome::Deleted,
        412 => PushOutcome::Conflict { local_etag },
        other => {
            return Err(SyncError::Http {
                status: other,
                message: format!("unexpected DELETE status for uid {uid}"),
            })
        }
    };
    Ok(PushResult {
        uid: uid.to_string(),
        outcome,
    })
}

/// Assert a response is a `207 Multi-Status`, else raise a protocol/HTTP error.
fn expect_multistatus(resp: &crate::transport::HttpResponse) -> SyncResult<()> {
    if resp.status == 207 {
        Ok(())
    } else {
        Err(SyncError::Http {
            status: resp.status,
            message: "expected 207 Multi-Status".to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// DAV Multi-Status parsing (namespace-tolerant: matched on local names).
// ---------------------------------------------------------------------------

/// A parsed `<d:multistatus>` body.
#[derive(Debug, Default)]
struct MultiStatus {
    /// The collection-level `<d:sync-token>` (RFC 6578), if present.
    sync_token: Option<String>,
    responses: Vec<DavResponse>,
}

/// One `<d:response>` element.
#[derive(Debug, Default)]
struct DavResponse {
    href: String,
    /// A response-level `<d:status>` (used for 404 sync tombstones).
    status: Option<u16>,
    props: DavProps,
}

/// The union of properties we extract from a response's `<d:propstat>`.
#[derive(Debug, Default)]
struct DavProps {
    displayname: Option<String>,
    getetag: Option<String>,
    getctag: Option<String>,
    calendar_data: Option<String>,
    color: Option<String>,
    sync_token: Option<String>,
    is_calendar: bool,
    /// The `<d:propstat>/<d:status>` code (e.g. 200 or 404).
    propstat_status: Option<u16>,
}

/// The local (namespace-stripped), lowercased name of an element.
fn local_of(name: QName<'_>) -> String {
    String::from_utf8_lossy(name.local_name().as_ref()).to_ascii_lowercase()
}

/// Extract the first 3-digit code from an HTTP status line (`HTTP/1.1 404 ...`).
fn parse_status_code(s: &str) -> Option<u16> {
    s.split_whitespace()
        .find_map(|t| t.parse::<u16>().ok().filter(|n| (100..=599).contains(n)))
}

/// Parse a `Multi-Status` body into [`MultiStatus`]. Tolerant of namespace
/// prefixes (matches on local names) and of servers that omit optional elements.
///
/// Text is **accumulated** across `Text` and `GeneralRef` events (quick-xml 0.41
/// splits an element's character data at every `&entity;`) and flushed on the
/// closing tag — so values containing XML entities (e.g. `Ben &amp; Jerry`) are
/// reassembled intact.
fn parse_multistatus(xml: &str) -> SyncResult<MultiStatus> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut ms = MultiStatus::default();
    let mut cur: Option<DavResponse> = None;
    let mut in_resourcetype = false;
    let mut in_propstat = false;
    // Whether this response's href was already captured (first one wins).
    let mut href_captured = false;
    // Accumulated character data of the currently open leaf element.
    let mut text_buf = String::new();

    loop {
        let ev = reader
            .read_event()
            .map_err(|e| SyncError::Xml(e.to_string()))?;
        match ev {
            Event::Start(e) => {
                text_buf.clear();
                match local_of(e.name()).as_str() {
                    "response" => {
                        cur = Some(DavResponse::default());
                        href_captured = false;
                    }
                    "resourcetype" => in_resourcetype = true,
                    "propstat" => in_propstat = true,
                    _ => {}
                }
            }
            Event::Empty(e) => {
                if local_of(e.name()) == "calendar" && in_resourcetype {
                    if let Some(r) = cur.as_mut() {
                        r.props.is_calendar = true;
                    }
                }
            }
            Event::Text(t) => {
                text_buf.push_str(
                    &t.xml_content(XmlVersion::Implicit1_0)
                        .map_err(|e| SyncError::Xml(e.to_string()))?,
                );
            }
            Event::GeneralRef(r) => {
                // Resolve `&amp;`/`&#65;`/… back to its character(s); ignore any
                // non-predefined custom entity rather than failing the whole parse.
                if let Ok(resolved) = r.xml_content(XmlVersion::Implicit1_0) {
                    text_buf.push_str(&resolved);
                }
            }
            Event::End(e) => {
                let name = local_of(e.name());
                let text = std::mem::take(&mut text_buf);
                match name.as_str() {
                    "href" if cur.is_some() && !href_captured => {
                        if let Some(r) = cur.as_mut() {
                            r.href = text;
                            href_captured = true;
                        }
                    }
                    "displayname" => set_prop(&mut cur, |p| p.displayname = Some(text)),
                    "getetag" => set_prop(&mut cur, |p| p.getetag = Some(text)),
                    "getctag" => set_prop(&mut cur, |p| p.getctag = Some(text)),
                    "calendar-data" => set_prop(&mut cur, |p| p.calendar_data = Some(text)),
                    "calendar-color" => set_prop(&mut cur, |p| p.color = Some(text)),
                    "status" => {
                        let code = parse_status_code(&text);
                        if in_propstat {
                            set_prop(&mut cur, |p| p.propstat_status = code);
                        } else if let Some(r) = cur.as_mut() {
                            r.status = code;
                        }
                    }
                    "sync-token" => {
                        if cur.is_some() {
                            set_prop(&mut cur, |p| p.sync_token = Some(text));
                        } else {
                            // Collection-level token (direct child of multistatus).
                            ms.sync_token = Some(text);
                        }
                    }
                    "response" => {
                        if let Some(r) = cur.take() {
                            ms.responses.push(r);
                        }
                    }
                    "resourcetype" => in_resourcetype = false,
                    "propstat" => in_propstat = false,
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(ms)
}

/// Apply `f` to the current response's props, if a response is open.
fn set_prop(cur: &mut Option<DavResponse>, f: impl FnOnce(&mut DavProps)) {
    if let Some(r) = cur.as_mut() {
        f(&mut r.props);
    }
}

// ---------------------------------------------------------------------------
// URL helpers (kept minimal; no url crate).
// ---------------------------------------------------------------------------

/// The `scheme://host[:port]` origin of an absolute URL, if it is one.
fn origin_of(url: &str) -> Option<&str> {
    let scheme_end = url.find("://")?;
    let after = scheme_end + 3;
    let rest = &url[after..];
    let host_len = rest.find('/').unwrap_or(rest.len());
    Some(&url[..after + host_len])
}

/// Resolve `href` (absolute URL, absolute path, or relative) against `base`.
fn resolve_href(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    if let Some(rest) = href.strip_prefix('/') {
        if let Some(origin) = origin_of(base) {
            return format!("{origin}/{rest}");
        }
        return href.to_string();
    }
    // Relative to the collection URL.
    if base.ends_with('/') {
        format!("{base}{href}")
    } else {
        format!("{base}/{href}")
    }
}

/// Whether response href `a` addresses the same resource as base collection `b`,
/// ignoring a trailing slash (resolves `a` against `b` first, so a bare path
/// matches the full base URL).
fn hrefs_equal(a: &str, b: &str) -> bool {
    let a_full = resolve_href(b, a);
    a_full.trim_end_matches('/') == b.trim_end_matches('/')
}

/// Percent-encode a single URL path segment (RFC 3986 unreserved set kept raw).
fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
