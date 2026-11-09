//! The whole server, built as the binary builds it, over HTTP: sign-in is required, and a client
//! with the owner's token sees the Revolut tools at /revolut and /mcp.

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use http_body_util::BodyExt;
use mcp_server_core::service::{HttpSettings, ServiceContext, ServiceEnv};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const HOST: &str = "bank-mcp.example.com";
const TOKEN: &str = "e2e-static-token-0123456789abcdef0123456789abcdef";
const KEY: &str =
    include_str!("../../mcp-revolut-business-banking/tests/fixtures/test-ed25519.pem");

fn server(extra: &[(&str, &str)]) -> axum::Router {
    let mut vars: HashMap<String, String> = [
        ("PUBLIC_URL", "https://bank-mcp.example.com"),
        ("OAUTH_SIGNING_KEY", "0123456789abcdef0123456789abcdef"),
        ("OAUTH_ADMIN_PASSWORD", "a long test password"),
        ("MCP_AUTH_TOKEN", TOKEN),
        ("INSECURE_HTTP", "true"),
        (
            "REVOLUT_API_KEY",
            "r_test_placeholder0000000000000000000000000000000000000000000000",
        ),
        ("REVOLUT_PRIVATE_KEY", KEY),
        // Keep the service off the network: nothing here reaches Revolut.
        ("REVOLUT_API_URL", "http://127.0.0.1:9/api/1.0"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    for (k, v) in extra {
        vars.insert(k.to_string(), v.to_string());
    }
    let mut env = ServiceEnv::new(Box::new(move |k: &str| vars.get(k).cloned()));
    let server = mcp_revolut_business::server();
    let loaded = server
        .load(&mut env)
        .unwrap_or_else(|| panic!("{:?}", env.errors));
    let ctx = ServiceContext {
        http: HttpSettings::from_core(&loaded.core),
        shutdown: CancellationToken::new(),
    };
    server.router(&loaded, &ctx)
}

fn rpc(path: &str, body: Value, token: Option<&str>, session: Option<&str>) -> Request<Body> {
    let mut b = Request::post(path)
        .header("host", HOST)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    if let Some(s) = session {
        b = b
            .header("mcp-session-id", s)
            .header("mcp-protocol-version", "2025-06-18");
    }
    b.body(Body::from(body.to_string())).unwrap()
}

/// Reads an SSE (or plain JSON) response until the first complete JSON-RPC message.
async fn rpc_message(res: Response<Body>) -> Value {
    let mut body = res.into_body();
    let mut buf = String::new();
    loop {
        if let Ok(v) = serde_json::from_str::<Value>(buf.trim()) {
            return v;
        }
        for line in buf.lines() {
            if let Some(data) = line.strip_prefix("data:")
                && let Ok(v) = serde_json::from_str::<Value>(data.trim())
                && v.get("jsonrpc").is_some()
            {
                return v;
            }
        }
        let frame = tokio::time::timeout(Duration::from_secs(10), body.frame())
            .await
            .expect("response within 10 s")
            .expect("body not finished before a message")
            .expect("body frame");
        if let Ok(data) = frame.into_data() {
            buf.push_str(&String::from_utf8_lossy(&data));
        }
    }
}

/// Opens an MCP session at `path` and returns its id.
async fn session(app: &axum::Router, path: &str) -> String {
    let res = app
        .clone()
        .oneshot(rpc(
            path,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "e2e", "version": "1"}
            }}),
            Some(TOKEN),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let id = res.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_string();
    let init = rpc_message(res).await;
    let want = if path == "/mcp" {
        "mcp-revolut-business"
    } else {
        "mcp-revolut-business-banking"
    };
    assert_eq!(init["result"]["serverInfo"]["name"], want);
    let res = app
        .clone()
        .oneshot(rpc(
            path,
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            Some(TOKEN),
            Some(&id),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    id
}

async fn call(app: &axum::Router, path: &str, sid: &str, method: &str, params: Value) -> Value {
    let res = app
        .clone()
        .oneshot(rpc(
            path,
            json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}),
            Some(TOKEN),
            Some(sid),
        ))
        .await
        .unwrap();
    rpc_message(res).await
}

#[tokio::test]
async fn healthz_needs_no_auth() {
    let res = server(&[])
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_mcp_endpoints_need_sign_in() {
    for path in ["/revolut", "/mcp"] {
        let res = server(&[])
            .oneshot(rpc(
                path,
                json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{path}");
        let res = server(&[])
            .oneshot(rpc(
                path,
                json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
                Some("wrong"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
}

#[tokio::test]
async fn tools_and_prompts_are_served_and_payments_stay_hidden() {
    let app = server(&[]);
    let sid = session(&app, "/revolut").await;
    let list = call(&app, "/revolut", &sid, "tools/list", json!({})).await;
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"list_accounts") && names.contains(&"create_payment_draft"),
        "{names:?}"
    );
    assert!(!names.contains(&"create_payment"), "{names:?}");
    let prompts = call(&app, "/revolut", &sid, "prompts/list", json!({})).await;
    assert_eq!(prompts["result"]["prompts"].as_array().unwrap().len(), 4);
    // A tool call reaches the client; Revolut is unreachable here, so it reports upstream.
    let r = call(
        &app,
        "/revolut",
        &sid,
        "tools/call",
        json!({"name": "list_accounts", "arguments": {}}),
    )
    .await;
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("upstream:"),
        "{r}"
    );
}

#[tokio::test]
async fn the_combined_endpoint_prefixes_tools() {
    let app = server(&[("REVOLUT_ALLOW_PAYMENTS", "true")]);
    let sid = session(&app, "/mcp").await;
    let list = call(&app, "/mcp", &sid, "tools/list", json!({})).await;
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"revolut_create_payment"), "{names:?}");
}

#[tokio::test]
async fn read_only_leaves_only_reads() {
    let app = server(&[
        ("REVOLUT_READ_ONLY", "all"),
        ("REVOLUT_ALLOW_PAYMENTS", "true"),
    ]);
    let sid = session(&app, "/revolut").await;
    let list = call(&app, "/revolut", &sid, "tools/list", json!({})).await;
    for t in list["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["annotations"]["readOnlyHint"], true, "{}", t["name"]);
    }
}
