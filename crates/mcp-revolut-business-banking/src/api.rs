//! Thin client for the Revolut Business API: every request signed with the API key (see
//! [`crate::signing`]), no redirects, timeouts, one retry of reads after a transport error or a
//! 5xx, status mapping and metrics. Writes are never retried, so a slow payment cannot be sent
//! twice.

use crate::error::{Result, RvError};
use crate::signing::{SIGNATURE_HEADER, Signer, TIMESTAMP_HEADER};
use mcp_server_core::observability::content_logging_enabled;
use reqwest::header::RETRY_AFTER;
use reqwest::{Method, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use std::time::{Duration, Instant};
use url::Url;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const RETRY_DELAY: Duration = Duration::from_millis(250);
/// Largest response body read.
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_ERROR_DETAIL: usize = 400;
/// Longest raw API path accepted.
const MAX_PATH_CHARS: usize = 200;

pub struct ApiSettings {
    /// The API base, ending in `/api/1.0`.
    pub base: Url,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub retry_delay: Duration,
}

impl ApiSettings {
    pub fn new(base: Url) -> Self {
        ApiSettings {
            base,
            connect_timeout: CONNECT_TIMEOUT,
            request_timeout: REQUEST_TIMEOUT,
            retry_delay: RETRY_DELAY,
        }
    }
}

/// The HTTP client every Revolut request goes through: no redirects, timeouts, this server's user
/// agent.
pub fn http_client(settings: &ApiSettings) -> Result<reqwest::Client> {
    mcp_server_core::install_crypto_provider();
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(settings.connect_timeout)
        .timeout(settings.request_timeout)
        .user_agent(format!(
            "mcp-revolut-business/{} (+https://github.com/Trozz/mcp-revolut-business)",
            crate::VERSION
        ))
        .build()
        .map_err(|e| RvError::Internal(format!("building the HTTP client failed: {e}")))
}

pub struct RvClient {
    http: reqwest::Client,
    settings: ApiSettings,
    api_key: SecretString,
    signer: Signer,
}

impl std::fmt::Debug for RvClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RvClient")
            .field("base", &self.settings.base.as_str())
            .finish_non_exhaustive()
    }
}

fn status_class(status: Option<StatusCode>) -> &'static str {
    match status.map(|s| s.as_u16()) {
        Some(200..=299) => "2xx",
        Some(300..=399) => "3xx",
        Some(400..=499) => "4xx",
        Some(500..=599) => "5xx",
        _ => "error",
    }
}

/// A short, single-line message from a Revolut error body: `{"code": 3000, "message": "..."}`,
/// an OAuth `{"error": ..., "error_description": ...}`, or plain text.
pub fn error_detail(body: &str) -> Option<String> {
    let text = match serde_json::from_str::<Value>(body) {
        Ok(v) => {
            let message = v
                .get("message")
                .or_else(|| v.get("error_description"))
                .or_else(|| v.get("error"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let code = v.get("code").and_then(|c| match c {
                Value::Number(n) => Some(n.to_string()),
                Value::String(s) => Some(s.clone()),
                _ => None,
            });
            match (message, code) {
                (Some(m), Some(c)) => format!("{m} (code {c})"),
                (Some(m), None) => m,
                (None, _) => return None,
            }
        }
        Err(_) => {
            let t = body.trim();
            if t.is_empty() || t.starts_with('<') {
                return None;
            }
            t.to_string()
        }
    };
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.is_empty() {
        return None;
    }
    if joined.chars().count() > MAX_ERROR_DETAIL {
        let cut: String = joined.chars().take(MAX_ERROR_DETAIL).collect();
        return Some(format!("{cut}..."));
    }
    Some(joined)
}

fn map_status(
    method: &Method,
    path: &str,
    status: StatusCode,
    body: &str,
    retry_after_secs: Option<u64>,
) -> RvError {
    let detail = error_detail(body);
    let with_detail = |prefix: String| match &detail {
        Some(d) => format!("{prefix}: {d}"),
        None => prefix,
    };
    match status.as_u16() {
        401 => RvError::Auth(detail.unwrap_or_else(|| "HTTP 401".into())),
        403 => RvError::Permission(with_detail(format!(
            "Revolut Business refused {method} {path} (HTTP 403): the API key's access, or its IP allowlist, does not allow it"
        ))),
        404 => RvError::NotFound(with_detail(format!("nothing found at {path}"))),
        429 => RvError::RateLimited { retry_after_secs },
        400 | 405 | 409 | 422 => RvError::Validation(with_detail(format!(
            "Revolut Business rejected {method} {path} (HTTP {})",
            status.as_u16()
        ))),
        300..=399 => RvError::Upstream(format!(
            "{method} {path} returned an unexpected redirect (HTTP {})",
            status.as_u16()
        )),
        code => RvError::Upstream(with_detail(format!("{method} {path} returned HTTP {code}"))),
    }
}

impl RvClient {
    pub fn new(
        http: reqwest::Client,
        settings: ApiSettings,
        api_key: SecretString,
        signer: Signer,
    ) -> Self {
        RvClient {
            http,
            settings,
            api_key,
            signer,
        }
    }

    pub fn base(&self) -> &Url {
        &self.settings.base
    }

    /// `base` with `segments` appended (each percent-encoded) and `query` added.
    pub fn url(&self, segments: &[&str], query: &[(&str, String)]) -> Url {
        let mut url = self.settings.base.clone();
        url.path_segments_mut()
            .expect("an http(s) base URL has path segments")
            .pop_if_empty()
            .extend(segments);
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in query {
                pairs.append_pair(k, v);
            }
        }
        url
    }

    /// The path segments below the API base of a raw path given by a person or model, such as
    /// `accounts/abc` or `/api/1.0/accounts`. The auth endpoints are refused.
    pub fn raw_segments(&self, raw: &str) -> Result<Vec<String>> {
        let raw = raw.trim();
        let base = &self.settings.base;
        let path = if let Ok(u) = Url::parse(raw) {
            if u.origin() != base.origin() {
                return Err(RvError::invalid(format!(
                    "path must be on this server's Revolut Business API ({}), not another host",
                    base.origin().ascii_serialization()
                )));
            }
            if u.query().is_some() {
                return Err(RvError::invalid(
                    "put query parameters in query, not in path",
                ));
            }
            u.path().to_string()
        } else {
            raw.to_string()
        };
        if path.contains('?') || path.contains('#') {
            return Err(RvError::invalid(
                "put query parameters in query, not in path",
            ));
        }
        let base_path = base.path().trim_end_matches('/');
        let rest = path
            .strip_prefix(base_path)
            .filter(|r| r.is_empty() || r.starts_with('/'))
            .unwrap_or(&path);
        let rest = rest.strip_prefix("/api/1.0/").unwrap_or(rest);
        let segments: Vec<String> = rest
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        let ok = !segments.is_empty()
            && rest.chars().count() <= MAX_PATH_CHARS
            && segments.iter().all(|s| {
                s != "."
                    && s != ".."
                    && s.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            })
            && segments[0] != "auth";
        if !ok {
            return Err(RvError::invalid(
                "path must be a Revolut Business API path such as accounts or transactions/<id>",
            ));
        }
        Ok(segments)
    }

    /// GET a JSON resource. `endpoint` is a fixed label for metrics and logs.
    pub async fn get(
        &self,
        endpoint: &'static str,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<Value> {
        self.send(Method::GET, endpoint, segments, query, None)
            .await
    }

    /// Sends a signed request and returns the JSON answer (`null` for an empty one).
    pub async fn send(
        &self,
        method: Method,
        endpoint: &'static str,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value> {
        let url = self.url(segments, query);
        let path = url.path().to_string();
        // Compact JSON: Revolut checks the signature against the body as it reads it.
        let body = body.map(|b| b.to_string());
        let retryable = method == Method::GET;
        let mut retried = false;
        loop {
            let started = Instant::now();
            let sig = self.signer.sign(
                method.as_str(),
                url.path(),
                url.query().unwrap_or(""),
                body.as_deref().unwrap_or("").as_bytes(),
            );
            let mut request = self
                .http
                .request(method.clone(), url.clone())
                .bearer_auth(self.api_key.expose_secret())
                .header(TIMESTAMP_HEADER, &sig.timestamp)
                .header(SIGNATURE_HEADER, &sig.signature)
                .header(reqwest::header::ACCEPT, "application/json");
            if let Some(body) = &body {
                request = request
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(body.clone());
            }
            let result = request.send().await;
            let status = result.as_ref().ok().map(|r| r.status());
            metrics::counter!("revolut_requests_total", "endpoint" => endpoint, "status_class" => status_class(status))
                .increment(1);
            metrics::histogram!("revolut_request_duration_seconds", "endpoint" => endpoint)
                .record(started.elapsed().as_secs_f64());
            tracing::debug!(
                endpoint,
                method = %method,
                path = %path,
                status = status.map(|s| s.as_u16()),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "revolut request"
            );
            let response = match result {
                Ok(r) => r,
                Err(e) if retryable && !retried => {
                    tracing::debug!(endpoint, error = %describe(&e), "retrying");
                    retried = true;
                    tokio::time::sleep(self.settings.retry_delay).await;
                    continue;
                }
                Err(e) => {
                    let what = if e.is_timeout() {
                        "timed out"
                    } else {
                        "failed"
                    };
                    let mut msg = format!("{method} {path} {what}: {}", describe(&e));
                    if !retryable {
                        msg.push_str(
                            "; it may or may not have been carried out, so check (list_transactions) before trying again",
                        );
                    }
                    return Err(RvError::Upstream(msg));
                }
            };
            let status = response.status();
            if status.is_server_error() && retryable && !retried {
                tracing::debug!(endpoint, status = status.as_u16(), "retrying");
                retried = true;
                tokio::time::sleep(self.settings.retry_delay).await;
                continue;
            }
            let retry_after_secs = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok());
            let text = read_body(response, &path).await?;
            if content_logging_enabled() {
                tracing::debug!(endpoint, path = %path, status = status.as_u16(), body = %text, "revolut response");
            }
            if !status.is_success() {
                if status == StatusCode::TOO_MANY_REQUESTS {
                    metrics::counter!("revolut_rate_limited_total").increment(1);
                }
                return Err(map_status(&method, &path, status, &text, retry_after_secs));
            }
            return if text.trim().is_empty() {
                Ok(Value::Null)
            } else {
                serde_json::from_str(&text).map_err(|e| {
                    RvError::Upstream(format!(
                        "{method} {path} returned invalid JSON ({e}); check REVOLUT_API_URL points at the Revolut Business API"
                    ))
                })
            };
        }
    }
}

fn describe(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    // reqwest includes the URL, which carries query values; keep only the kind of failure.
    msg.split(" for url ").next().unwrap_or(&msg).to_string()
}

async fn read_body(mut response: reqwest::Response, path: &str) -> Result<String> {
    let too_big = || {
        RvError::Upstream(format!(
            "{path} answered with more than {} MB",
            MAX_BODY_BYTES / (1024 * 1024)
        ))
    };
    if response
        .content_length()
        .is_some_and(|n| n as usize > MAX_BODY_BYTES)
    {
        return Err(too_big());
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| RvError::Upstream(format!("reading {path} failed: {}", describe(&e))))?
    {
        if buf.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(too_big());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_bodies_in_every_shape() {
        assert_eq!(
            error_detail(r#"{"code":3000,"message":"Insufficient balance"}"#).as_deref(),
            Some("Insufficient balance (code 3000)")
        );
        assert_eq!(
            error_detail(r#"{"error":"invalid_grant","error_description":"expired"}"#).as_deref(),
            Some("expired")
        );
        assert_eq!(
            error_detail("Too many requests").as_deref(),
            Some("Too many requests")
        );
        assert_eq!(error_detail("<html>oops</html>"), None);
        assert_eq!(error_detail(""), None);
    }

    #[test]
    fn statuses_map_to_kinds() {
        let m = Method::GET;
        for (status, kind) in [
            (StatusCode::UNAUTHORIZED, "auth"),
            (StatusCode::FORBIDDEN, "permission"),
            (StatusCode::NOT_FOUND, "not_found"),
            (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            (StatusCode::BAD_REQUEST, "validation"),
            (StatusCode::UNPROCESSABLE_ENTITY, "validation"),
            (StatusCode::BAD_GATEWAY, "upstream"),
        ] {
            assert_eq!(map_status(&m, "/api/1.0/x", status, "", None).kind(), kind);
        }
        let e = map_status(
            &Method::POST,
            "/api/1.0/pay",
            StatusCode::BAD_REQUEST,
            r#"{"code":3000,"message":"Insufficient balance"}"#,
            None,
        );
        assert!(e.to_string().contains("Insufficient balance"), "{e}");
    }
}
