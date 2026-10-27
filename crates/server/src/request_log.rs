//! One log line per HTTP request that needs attention, for debugging the hosted
//! control plane from its logs.
//!
//! Every request runs in a span carrying a request id (also returned as
//! `x-request-id`), the method and path (never the query string), and, once auth
//! has run, the org and the signed-in login. Responses are logged at:
//!
//! - `error`: 5xx, and panics (caught and turned into 500s);
//! - `warn`: 4xx, with the error message the caller was sent;
//! - `info`: anything slower than [`SLOW`], except long polls;
//! - `debug`: everything else (`RUST_LOG=sidefleet_server::request_log=debug`).

use std::any::Any;
use std::time::Duration;

use axum::{
    body::Body,
    http::{header::HeaderName, Request, Response, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::json;
use tower_http::classify::ServerErrorsFailureClass;
use tower_http::trace::{MakeSpan, OnFailure, OnResponse};
use tracing::{field::Empty, Span};

pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
/// Requests slower than this are logged even when they succeed.
const SLOW: Duration = Duration::from_secs(2);
/// Held open on purpose until there is something to return.
const LONG_POLLS: &[&str] = &["/v1/worker/claim"];

/// The message an error response carried, attached so the response log can show it.
#[derive(Clone)]
pub struct ErrorMessage(pub String);

/// A `{"error": msg}` response that the request log reports with its message.
pub fn error_response(status: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    let msg = msg.into();
    let mut res = (status, Json(json!({ "error": msg }))).into_response();
    res.extensions_mut().insert(ErrorMessage(msg));
    res
}

/// Record who is calling on the current request's span (called by the auth middleware).
pub fn record_caller(org: uuid::Uuid, login: Option<&str>) {
    let span = Span::current();
    span.record("org", tracing::field::display(org));
    if let Some(login) = login {
        span.record("user", login);
    }
}

#[derive(Clone, Copy)]
pub struct RequestSpan;

impl<B> MakeSpan<B> for RequestSpan {
    fn make_span(&mut self, req: &Request<B>) -> Span {
        let id = req.headers().get(REQUEST_ID).and_then(|v| v.to_str().ok()).unwrap_or("");
        let (method, path) = (req.method(), req.uri().path());
        if LONG_POLLS.contains(&path) {
            tracing::info_span!("long_poll", id, %method, path, org = Empty, user = Empty)
        } else {
            tracing::info_span!("request", id, %method, path, org = Empty, user = Empty)
        }
    }
}

#[derive(Clone, Copy)]
pub struct LogResponse;

impl<B> OnResponse<B> for LogResponse {
    fn on_response(self, res: &Response<B>, latency: Duration, span: &Span) {
        let _enter = span.enter();
        let status = res.status().as_u16();
        let ms = latency.as_millis() as u64;
        let error = res.extensions().get::<ErrorMessage>().map(|m| m.0.as_str());
        if res.status().is_server_error() {
            tracing::error!(status, ms, error, "request failed");
        } else if res.status().is_client_error() {
            tracing::warn!(status, ms, error, "request refused");
        } else if latency > SLOW && span.metadata().is_some_and(|m| m.name() != "long_poll") {
            tracing::info!(status, ms, "slow request");
        } else {
            tracing::debug!(status, ms, "request");
        }
    }
}

/// Errors other than a 5xx status (those are logged by [`LogResponse`]), such as a body failing mid-stream.
#[derive(Clone, Copy)]
pub struct LogFailure;

impl OnFailure<ServerErrorsFailureClass> for LogFailure {
    fn on_failure(&mut self, class: ServerErrorsFailureClass, latency: Duration, span: &Span) {
        if let ServerErrorsFailureClass::Error(e) = class {
            let _enter = span.enter();
            tracing::error!(error = %e, ms = latency.as_millis() as u64, "response failed");
        }
    }
}

/// A handler panicked: answer 500 instead of dropping the connection. The panic itself is logged by the panic hook.
pub fn panic_response(err: Box<dyn Any + Send + 'static>) -> Response<Body> {
    let msg = err.downcast_ref::<String>().cloned().or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
    let mut res = error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    res.extensions_mut().insert(ErrorMessage(format!("panic: {msg}")));
    res
}
