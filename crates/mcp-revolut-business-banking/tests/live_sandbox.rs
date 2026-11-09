//! The tools against Revolut's real sandbox. Skipped unless a sandbox key is given:
//!
//! ```sh
//! REVOLUT_SANDBOX_API_KEY=r_test_... REVOLUT_SANDBOX_PRIVATE_KEY_FILE=key.pem \
//!   cargo test -p mcp-revolut-business-banking --test live_sandbox -- --test-threads 1
//! ```
//!
//! It reads accounts, transactions, rates, counterparties, drafts, cards and team members, and
//! saves then deletes one counterparty. Nothing moves money.
mod support;

use mcp_revolut_business_banking::api::{ApiSettings, RvClient, http_client};
use mcp_revolut_business_banking::config::SANDBOX_API_URL;
use mcp_revolut_business_banking::signing::Signer;
use mcp_revolut_business_banking::tools::BankingServer;
use serde_json::json;
use std::sync::Arc;
use support::tools::{json, params};

fn sandbox() -> Option<BankingServer> {
    let key = std::env::var("REVOLUT_SANDBOX_API_KEY").ok()?;
    let pem = std::fs::read_to_string(std::env::var("REVOLUT_SANDBOX_PRIVATE_KEY_FILE").ok()?)
        .expect("REVOLUT_SANDBOX_PRIVATE_KEY_FILE is readable");
    assert!(
        key.starts_with("r_test_"),
        "only a sandbox key may be used here"
    );
    let settings = ApiSettings::new(url::Url::parse(SANDBOX_API_URL).unwrap());
    let http = http_client(&settings).unwrap();
    let client = RvClient::new(http, settings, key.into(), Signer::from_pem(&pem).unwrap());
    Some(BankingServer::new(Arc::new(client), false))
}

macro_rules! skip_without_sandbox {
    () => {
        match sandbox() {
            Some(s) => s,
            None => {
                eprintln!(
                    "skipped: set REVOLUT_SANDBOX_API_KEY and REVOLUT_SANDBOX_PRIVATE_KEY_FILE"
                );
                return;
            }
        }
    };
}

#[tokio::test]
async fn reads_the_sandbox_business() {
    let s = skip_without_sandbox!();
    let accounts = json(&s.list_accounts(params(json!({}))).await.unwrap());
    assert!(accounts["count"].as_u64().unwrap() > 0, "{accounts}");
    let id = accounts["items"][0]["id"].as_str().unwrap().to_string();
    json(
        &s.get_account(params(json!({"account_id": id})))
            .await
            .unwrap(),
    );
    json(
        &s.get_account_bank_details(params(json!({"account_id": id})))
            .await
            .unwrap(),
    );
    let snap = json(&s.business_snapshot(params(json!({}))).await.unwrap());
    assert!(snap["balances_by_currency"].is_object(), "{snap}");
    let tx = json(
        &s.list_transactions(params(json!({"count": 5})))
            .await
            .unwrap(),
    );
    if let Some(t) = tx["items"][0]["id"].as_str() {
        json(
            &s.get_transaction(params(json!({"transaction_id": t})))
                .await
                .unwrap(),
        );
    }
    json(
        &s.list_transactions(params(
            json!({"from": "2026-01-01", "account_id": id, "type": "topup"}),
        ))
        .await
        .unwrap(),
    );
    let rate = json(
        &s.get_exchange_rate(params(json!({"from": "GBP", "to": "EUR", "amount": "10"})))
            .await
            .unwrap(),
    );
    assert!(rate["rate"].as_f64().unwrap() > 0.0, "{rate}");
    json(
        &s.list_counterparties(params(json!({"count": 5})))
            .await
            .unwrap(),
    );
    json(&s.list_payment_drafts(params(json!({}))).await.unwrap());
    json(&s.list_cards(params(json!({}))).await.unwrap());
    json(&s.list_team_members(params(json!({}))).await.unwrap());
    json(
        &s.list_payout_links(params(json!({"count": 5})))
            .await
            .unwrap(),
    );
    json(
        &s.api_get(params(json!({"path": "counterparties/countries"})))
            .await
            .unwrap(),
    );
}

#[tokio::test]
async fn saves_and_deletes_a_counterparty() {
    let s = skip_without_sandbox!();
    let made = json(
        &s.create_counterparty(params(json!({
            "company_name": "MCP Live Test Ltd", "account_no": "12345678", "sort_code": "123456",
            "bank_country": "GB", "currency": "GBP"
        })))
        .await
        .unwrap(),
    );
    let id = made["id"].as_str().expect("an id").to_string();
    let got = json(
        &s.get_counterparty(params(json!({"counterparty_id": id})))
            .await
            .unwrap(),
    );
    assert_eq!(got["name"], "MCP Live Test Ltd");
    // A draft for it, deleted unsent: nothing is paid.
    let accounts = json(&s.list_accounts(params(json!({}))).await.unwrap());
    let gbp = accounts["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["currency"] == "GBP")
        .and_then(|a| a["id"].as_str())
        .expect("a GBP account")
        .to_string();
    let draft = json(
        &s.create_payment_draft(params(json!({"title": "MCP live test", "payments": [
            {"account_id": gbp, "counterparty_id": id, "amount": "1.00", "currency": "GBP", "reference": "MCP test"}
        ]})))
        .await
        .unwrap(),
    );
    let draft_id = draft["id"].as_str().expect("a draft id").to_string();
    // The sandbox does not always serve a draft back straight after creating it.
    let read = s
        .get_payment_draft(params(json!({"draft_id": draft_id})))
        .await
        .unwrap();
    if read.is_error == Some(true) {
        eprintln!("the sandbox did not serve the new draft back; skipping its read and delete");
    } else {
        assert_eq!(json(&read)["title"], "MCP live test");
        json(
            &s.delete_payment_draft(params(json!({"draft_id": draft_id})))
                .await
                .unwrap(),
        );
    }
    let gone = json(
        &s.delete_counterparty(params(json!({"counterparty_id": id})))
            .await
            .unwrap(),
    );
    assert_eq!(gone, json!({"ok": true}));
}
