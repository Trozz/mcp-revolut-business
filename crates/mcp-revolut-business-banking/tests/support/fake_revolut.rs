//! An in-memory Revolut Business API that checks requests as the real one does: the API key as a
//! bearer token, and an Ed25519 signature over timestamp, method, path, query and body. Tests set
//! canned answers per method and path, and every request is logged for assertions.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use ring::signature::{ED25519, UnparsedPublicKey};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use url::Url;

pub const API_KEY: &str = "r_test_placeholder0000000000000000000000000000000000000000000000";
pub const PRIVATE_KEY: &str = include_str!("../fixtures/test-ed25519.pem");
const PUBLIC_KEY: &str = include_str!("../fixtures/test-ed25519-public.pem");

#[derive(Debug, Clone)]
pub struct Logged {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Option<Value>,
}

impl Logged {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    pub fn params(&self, name: &str) -> Vec<&str> {
        self.query
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct Canned {
    pub status: u16,
    pub body: Value,
}

impl Canned {
    pub fn ok(body: Value) -> Self {
        Canned { status: 200, body }
    }
    pub fn status(status: u16, body: Value) -> Self {
        Canned { status, body }
    }
}

#[derive(Default)]
pub struct FakeState {
    pub log: Vec<Logged>,
    /// Answers by "METHOD /api/1.0/path"; a list is served in order, the last one repeatedly.
    pub canned: HashMap<String, Vec<Canned>>,
    pub refused_signatures: usize,
}

#[derive(Clone)]
pub struct FakeRevolut {
    pub url: Url,
    pub state: Arc<Mutex<FakeState>>,
}

fn public_key() -> Vec<u8> {
    let b64: String = PUBLIC_KEY
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();
    der[der.len() - 32..].to_vec()
}

impl FakeRevolut {
    pub async fn start() -> Self {
        let state = Arc::new(Mutex::new(FakeState::default()));
        let app = axum::Router::new()
            .fallback(handle)
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        FakeRevolut {
            url: Url::parse(&format!("http://{addr}/api/1.0")).unwrap(),
            state,
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().unwrap()
    }

    pub fn on(&self, method: &str, path: &str, answer: Canned) {
        self.lock()
            .canned
            .insert(format!("{method} /api/1.0/{path}"), vec![answer]);
    }

    pub fn on_seq(&self, method: &str, path: &str, answers: Vec<Canned>) {
        self.lock()
            .canned
            .insert(format!("{method} /api/1.0/{path}"), answers);
    }

    pub fn log(&self) -> Vec<Logged> {
        self.lock().log.clone()
    }

    pub fn last(&self, method: &str, path: &str) -> Logged {
        let full = format!("/api/1.0/{path}");
        self.lock()
            .log
            .iter()
            .rev()
            .find(|l| l.method == method && l.path == full)
            .cloned()
            .unwrap_or_else(|| panic!("no {method} {full} was made"))
    }

    pub fn count(&self, method: &str, path: &str) -> usize {
        let full = format!("/api/1.0/{path}");
        self.lock()
            .log
            .iter()
            .filter(|l| l.method == method && l.path == full)
            .count()
    }
}

fn json_response(status: u16, body: &Value) -> Response {
    (
        StatusCode::from_u16(status).unwrap(),
        [("content-type", "application/json")],
        if body.is_null() {
            String::new()
        } else {
            body.to_string()
        },
    )
        .into_response()
}

async fn handle(
    State(state): State<Arc<Mutex<FakeState>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_string();
    let raw_query = uri.query().unwrap_or("").to_string();
    let query: Vec<(String, String)> = url::form_urlencoded::parse(raw_query.as_bytes())
        .into_owned()
        .collect();
    let mut st = state.lock().unwrap();
    st.log.push(Logged {
        method: method.to_string(),
        path: path.clone(),
        query,
        body: serde_json::from_slice(&body).ok(),
    });
    let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or("");
    if headers.get("user-agent").is_none() {
        return json_response(400, &json!({"code": 400, "message": "no user agent"}));
    }
    if h("authorization") != format!("Bearer {API_KEY}") {
        return json_response(
            401,
            &json!({"code": 1000, "message": "Invalid token. The authorization header must follow the 'Bearer <token>' format."}),
        );
    }
    let ts = h("x-revx-timestamp");
    let sig = base64::engine::general_purpose::STANDARD
        .decode(h("x-revx-signature"))
        .unwrap_or_default();
    let mut message = format!("{ts}{method}{path}{raw_query}").into_bytes();
    message.extend_from_slice(&body);
    if ts.is_empty()
        || UnparsedPublicKey::new(&ED25519, public_key())
            .verify(&message, &sig)
            .is_err()
    {
        st.refused_signatures += 1;
        return json_response(
            401,
            &json!({"code": 1002, "message": "The provided signature is invalid. Ensure the signed message is the concatenation of timestamp + HTTP method + request path + query string + request body, with no separators."}),
        );
    }
    let key = format!("{method} {path}");
    if let Some(answers) = st.canned.get_mut(&key) {
        let a = if answers.len() > 1 {
            answers.remove(0)
        } else {
            answers[0].clone()
        };
        return json_response(a.status, &a.body);
    }
    json_response(404, &json!({"code": 3404, "message": "Not found"}))
}
