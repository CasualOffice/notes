//! The HTTP [`Transport`] seam and an in-memory [`MockTransport`] test double.
//!
//! See `docs/casual-note-calendar.md` §3. CalDAV protocol logic
//! ([`crate::caldav`]) is written entirely against this narrow async trait so the
//! crate stays free of any real HTTP client: the production transport (which
//! actually opens the single, user-consented socket to the user's own server, and
//! injects `Authorization` from the OS keystore) is supplied by the app-service as
//! a **documented seam**. This crate deliberately pulls in no heavy HTTP stack.
//!
//! Privacy invariant (doc §1, §7): the only place a socket may open in the whole
//! calendar surface is a concrete [`Transport`] impl for a CalDAV server the user
//! explicitly connected. Everything above this trait is pure and offline.

use std::collections::VecDeque;
use std::sync::Mutex;

use crate::error::{SyncError, SyncResult};

/// A single HTTP request, as the CalDAV client wants it issued.
///
/// Header names/values are kept as owned strings so a mock can assert on them and
/// a real transport can forward them verbatim. Bodies are UTF-8 XML or iCalendar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpRequest {
    /// HTTP/WebDAV method (`PROPFIND`, `REPORT`, `PUT`, `DELETE`, `GET`, …).
    pub method: String,
    /// Absolute request URL.
    pub url: String,
    /// Request headers, in insertion order (e.g. `Depth`, `If-Match`, `Content-Type`).
    pub headers: Vec<(String, String)>,
    /// Request body (empty for `DELETE`).
    pub body: String,
}

impl HttpRequest {
    /// Build a request with method + url; headers/body added fluently.
    #[must_use]
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            url: url.into(),
            headers: Vec::new(),
            body: String::new(),
        }
    }

    /// Append a header (chainable).
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Set the request body (chainable).
    #[must_use]
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    /// Case-insensitive lookup of the first header with `name`.
    #[must_use]
    pub fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A single HTTP response handed back to the CalDAV client.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP status code (e.g. `207` Multi-Status, `412` Precondition Failed).
    pub status: u16,
    /// Response headers (e.g. `ETag`, `DAV`).
    pub headers: Vec<(String, String)>,
    /// Response body (XML for `Multi-Status`, empty otherwise).
    pub body: String,
}

impl HttpResponse {
    /// Construct a response from a status and body (no headers).
    #[must_use]
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// Append a header (chainable).
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Case-insensitive lookup of the first header with `name`.
    #[must_use]
    pub fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Whether `status` is a 2xx success.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }
}

/// The single async I/O boundary of the calendar sync layer.
///
/// A concrete impl is the *only* component in the whole surface permitted to open
/// a socket, and only to the CalDAV server the user connected (doc §1). CalDAV
/// protocol code is generic over this trait, so all of it is unit-testable with
/// [`MockTransport`] and never touches the network.
// `async fn` in a trait is intentional: adapters are driven on a single task and
// need no `Send` future bound, so the ergonomic form is preferred over an
// `impl Future` desugaring here.
#[allow(async_fn_in_trait)]
pub trait Transport {
    /// Issue one request and await its response. A non-2xx status is returned as a
    /// successful [`HttpResponse`] (the CalDAV layer interprets 207/404/412/…);
    /// only a genuine I/O failure yields [`SyncError::Transport`].
    async fn request(&self, req: HttpRequest) -> SyncResult<HttpResponse>;
}

/// An in-memory [`Transport`] that records every request and replays canned
/// responses in FIFO order — the test double for all CalDAV protocol tests.
///
/// It opens no socket. If more requests arrive than responses were queued, the
/// next `request` returns [`SyncError::Transport`] so a mis-scripted test fails
/// loudly rather than hanging.
#[derive(Debug, Default)]
pub struct MockTransport {
    responses: Mutex<VecDeque<HttpResponse>>,
    captured: Mutex<Vec<HttpRequest>>,
}

impl MockTransport {
    /// Empty mock; queue responses with [`push_response`](Self::push_response).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a mock pre-loaded with `responses`, replayed in order.
    #[must_use]
    pub fn with_responses(responses: impl IntoIterator<Item = HttpResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            captured: Mutex::new(Vec::new()),
        }
    }

    /// Queue one more response at the back of the FIFO.
    pub fn push_response(&self, response: HttpResponse) {
        self.lock_responses().push_back(response);
    }

    /// Snapshot of every request issued so far, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The most recent request, if any.
    #[must_use]
    pub fn last_request(&self) -> Option<HttpRequest> {
        self.captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .cloned()
    }

    /// Number of requests issued so far.
    #[must_use]
    pub fn request_count(&self) -> usize {
        self.captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn lock_responses(&self) -> std::sync::MutexGuard<'_, VecDeque<HttpResponse>> {
        self.responses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Transport for MockTransport {
    async fn request(&self, req: HttpRequest) -> SyncResult<HttpResponse> {
        self.captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(req.clone());
        self.lock_responses().pop_front().ok_or_else(|| {
            SyncError::Transport(format!(
                "MockTransport: no canned response queued for {} {}",
                req.method, req.url
            ))
        })
    }
}
