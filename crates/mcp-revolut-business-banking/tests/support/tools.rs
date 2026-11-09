use super::fake_revolut::{API_KEY, FakeRevolut, PRIVATE_KEY};
use mcp_revolut_business_banking::api::{ApiSettings, RvClient, http_client};
use mcp_revolut_business_banking::signing::Signer;
use mcp_revolut_business_banking::tools::BankingServer;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::time::Duration;

pub fn client(fake: &FakeRevolut) -> Arc<RvClient> {
    let mut s = ApiSettings::new(fake.url.clone());
    s.retry_delay = Duration::ZERO;
    let http = http_client(&s).unwrap();
    Arc::new(RvClient::new(
        http,
        s,
        API_KEY.to_string().into(),
        Signer::from_pem(PRIVATE_KEY).unwrap(),
    ))
}

pub fn server(fake: &FakeRevolut) -> BankingServer {
    BankingServer::new(client(fake), false)
}

pub fn paying_server(fake: &FakeRevolut) -> BankingServer {
    BankingServer::new(client(fake), true)
}

pub fn params<T: DeserializeOwned>(v: serde_json::Value) -> Parameters<T> {
    Parameters(serde_json::from_value(v).expect("valid tool arguments"))
}

pub fn is_error(r: &CallToolResult) -> bool {
    r.is_error == Some(true)
}

pub fn text(r: &CallToolResult) -> String {
    r.content[0]
        .as_text()
        .expect("first content block is text")
        .text
        .clone()
}

pub fn json(r: &CallToolResult) -> serde_json::Value {
    assert!(!is_error(r), "tool error: {}", text(r));
    serde_json::from_str(&text(r)).expect("tool result is JSON")
}

pub fn error_text(r: &CallToolResult) -> String {
    assert!(is_error(r), "expected a tool error, got: {}", text(r));
    text(r)
}
