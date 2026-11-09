//! The tools against a fake Revolut Business that checks every request's signature.
mod support;

use serde_json::json;
use support::fake_revolut::{Canned, FakeRevolut};
use support::tools::{error_text, json, params, paying_server, server};

const GBP: &str = "1714b6ee-ca1a-e2cf-0040-00d3af294457";
const EUR: &str = "84d79450-bd2c-e211-0040-008cab314b30";
const CP: &str = "b6523744-b159-e2f1-0040-0001920ddaa0";

fn accounts() -> serde_json::Value {
    json!([
        {"id": GBP, "name": "Main", "balance": 289.10, "currency": "GBP", "state": "active", "public": false},
        {"id": EUR, "name": "European suppliers", "balance": 32.8, "currency": "EUR", "state": "active", "public": false},
        {"id": "d98a0bdc-5b4d-e2db-0040-0015908f444f", "name": null, "balance": 10, "currency": "GBP", "state": "active", "public": false},
        {"id": "old", "name": "Closed", "balance": 99, "currency": "USD", "state": "inactive", "public": false}
    ])
}

#[tokio::test]
async fn accounts_are_listed_through_signed_requests() {
    let fake = FakeRevolut::start().await;
    fake.on("GET", "accounts", Canned::ok(accounts()));
    let r = server(&fake)
        .list_accounts(params(json!({})))
        .await
        .unwrap();
    let v = json(&r);
    assert_eq!(v["count"], 4);
    assert!(v["items"][2].get("name").is_none(), "nulls are pruned");
    assert_eq!(fake.lock().refused_signatures, 0);
}

#[tokio::test]
async fn the_snapshot_sums_balances_and_survives_a_missing_section() {
    let fake = FakeRevolut::start().await;
    fake.on("GET", "accounts", Canned::ok(accounts()));
    fake.on(
        "GET",
        "transactions",
        Canned::ok(json!([
            {"id": "t1", "type": "card_payment", "state": "pending", "created_at": "2026-11-09T10:00:00Z",
             "merchant": {"name": "Coffee Ltd"}, "legs": [{"account_id": GBP, "amount": -3.5, "currency": "GBP", "description": "Coffee"}]},
            {"id": "t2", "type": "topup", "state": "completed", "created_at": "2026-11-08T10:00:00Z",
             "legs": [{"account_id": GBP, "amount": 100, "currency": "GBP"}]}
        ])),
    );
    fake.on(
        "GET",
        "payment-drafts",
        Canned::status(403, json!({"code": 9002, "message": "Forbidden"})),
    );
    let v = json(
        &server(&fake)
            .business_snapshot(params(json!({})))
            .await
            .unwrap(),
    );
    assert_eq!(
        v["balances_by_currency"],
        json!({"EUR": "32.80", "GBP": "299.10"})
    );
    assert_eq!(v["accounts"].as_array().unwrap().len(), 3);
    assert_eq!(v["pending_in_recent"], 1);
    assert_eq!(v["recent_transactions"][0]["merchant"], "Coffee Ltd");
    assert!(
        v["unavailable"]["payment_drafts"]
            .as_str()
            .unwrap()
            .starts_with("permission:")
    );
    assert_eq!(fake.last("GET", "transactions").param("count"), Some("10"));
}

#[tokio::test]
async fn transactions_are_filtered_and_say_how_to_get_older_ones() {
    let fake = FakeRevolut::start().await;
    fake.on(
        "GET",
        "transactions",
        Canned::ok(json!([
            {"id": "a", "created_at": "2026-11-02T00:00:00Z"},
            {"id": "b", "created_at": "2026-11-01T00:00:00Z"}
        ])),
    );
    let v = json(
        &server(&fake)
            .list_transactions(params(json!({
                "from": "2026-10-01", "to": "2026-11-05", "account_id": GBP,
                "type": "Card Payment", "states": ["completed", "pending"], "count": 2
            })))
            .await
            .unwrap(),
    );
    let l = fake.last("GET", "transactions");
    assert_eq!(l.param("from"), Some("2026-10-01"));
    assert_eq!(l.param("account"), Some(GBP));
    assert_eq!(l.param("type"), Some("card_payment"));
    assert_eq!(l.params("state"), vec!["completed", "pending"]);
    assert_eq!(v["more"], true);
    assert!(v["next"].as_str().unwrap().contains("2026-11-01T00:00:00Z"));
    assert_eq!(fake.lock().refused_signatures, 0);
}

#[tokio::test]
async fn bad_arguments_never_reach_revolut() {
    let fake = FakeRevolut::start().await;
    let s = server(&fake);
    for (r, needle) in [
        (
            s.list_transactions(params(json!({"from": "2026-12-01", "to": "2026-11-01"})))
                .await
                .unwrap(),
            "from must not be after to",
        ),
        (
            s.list_transactions(params(json!({"request_id": "abc"})))
                .await
                .unwrap(),
            "request_id needs from",
        ),
        (
            s.get_account(params(json!({"account_id": "../auth/token"})))
                .await
                .unwrap(),
            "account_id must be an id",
        ),
        (
            s.get_exchange_rate(params(json!({"from": "GBP", "to": "gbp"})))
                .await
                .unwrap(),
            "different currencies",
        ),
    ] {
        let t = error_text(&r);
        assert!(t.starts_with("validation:") && t.contains(needle), "{t}");
    }
    assert!(fake.log().is_empty());
}

#[tokio::test]
async fn revolut_errors_become_tool_errors_with_a_kind() {
    let fake = FakeRevolut::start().await;
    fake.on(
        "GET",
        &format!("accounts/{GBP}"),
        Canned::status(404, json!({"code": 3404, "message": "Account not found"})),
    );
    let t = error_text(
        &server(&fake)
            .get_account(params(json!({"account_id": GBP})))
            .await
            .unwrap(),
    );
    assert!(
        t.starts_with("not_found:") && t.contains("Account not found"),
        "{t}"
    );
    fake.on(
        "GET",
        "accounts",
        Canned::status(401, json!({"code": 1000, "message": "Invalid token"})),
    );
    let t = error_text(
        &server(&fake)
            .list_accounts(params(json!({})))
            .await
            .unwrap(),
    );
    assert!(
        t.starts_with("auth:") && t.contains("REVOLUT_API_KEY"),
        "{t}"
    );
}

#[tokio::test]
async fn reads_are_retried_once_after_a_server_error_but_writes_never() {
    let fake = FakeRevolut::start().await;
    fake.on_seq(
        "GET",
        "accounts",
        vec![Canned::status(503, json!({})), Canned::ok(accounts())],
    );
    json(
        &server(&fake)
            .list_accounts(params(json!({})))
            .await
            .unwrap(),
    );
    assert_eq!(fake.count("GET", "accounts"), 2);
    fake.on("POST", "payment-drafts", Canned::status(503, json!({})));
    let r = server(&fake)
        .create_payment_draft(params(json!({"payments": [
            {"account_id": GBP, "counterparty_id": CP, "amount": "5", "currency": "GBP", "reference": "x"}
        ]})))
        .await
        .unwrap();
    assert!(error_text(&r).starts_with("upstream:"));
    assert_eq!(fake.count("POST", "payment-drafts"), 1);
}

#[tokio::test]
async fn a_draft_is_signed_with_its_exact_body() {
    let fake = FakeRevolut::start().await;
    fake.on(
        "POST",
        "payment-drafts",
        Canned::status(201, json!({"id": "draft-1"})),
    );
    let v = json(
        &server(&fake)
            .create_payment_draft(params(json!({"title": "Suppliers", "payments": [
                {"account_id": GBP, "counterparty_id": CP, "amount": "1,250.50", "currency": "gbp", "reference": "INV-1042"}
            ]})))
            .await
            .unwrap(),
    );
    assert_eq!(v["id"], "draft-1");
    assert!(v["next"].as_str().unwrap().contains("approve"));
    let body = fake.last("POST", "payment-drafts").body.unwrap();
    assert_eq!(body["payments"][0]["amount"], json!(1250.5));
    assert_eq!(body["payments"][0]["currency"], "GBP");
    assert_eq!(fake.lock().refused_signatures, 0);
}

#[tokio::test]
async fn payments_are_hidden_and_refused_unless_allowed() {
    let fake = FakeRevolut::start().await;
    let off = server(&fake);
    assert!(!off.tool_names().contains(&"create_payment".to_string()));
    assert!(
        off.tool_names()
            .contains(&"create_payment_draft".to_string())
    );
    let pay = json!({"account_id": GBP, "counterparty_id": CP, "amount": "10", "currency": "GBP", "reference": "Rent"});
    let t = error_text(&off.create_payment(params(pay.clone())).await.unwrap());
    assert!(t.starts_with("disabled:"), "{t}");
    assert!(fake.log().is_empty());

    fake.on(
        "POST",
        "pay",
        Canned::ok(json!({"id": "tx-1", "state": "pending", "created_at": "2026-11-09T12:00:00Z"})),
    );
    let on = paying_server(&fake);
    let v = json(&on.create_payment(params(pay)).await.unwrap());
    assert_eq!(v["id"], "tx-1");
    let sent = fake.last("POST", "pay").body.unwrap();
    assert_eq!(
        v["request_id"], sent["request_id"],
        "the request id is returned for retries"
    );
    assert_eq!(sent["receiver"], json!({"counterparty_id": CP}));
    // A retry with the same request_id sends the same one, so Revolut pays once.
    json(
        &on.create_payment(params(json!({"account_id": GBP, "counterparty_id": CP, "amount": "10", "currency": "GBP", "request_id": sent["request_id"]})))
            .await
            .unwrap(),
    );
    assert_eq!(
        fake.last("POST", "pay").body.unwrap()["request_id"],
        sent["request_id"]
    );
}

#[tokio::test]
async fn transfers_and_exchanges() {
    let fake = FakeRevolut::start().await;
    fake.on(
        "POST",
        "transfer",
        Canned::ok(json!({"id": "t", "state": "completed"})),
    );
    fake.on(
        "POST",
        "exchange",
        Canned::ok(json!({"id": "e", "state": "completed"})),
    );
    let s = paying_server(&fake);
    json(
        &s.transfer_between_accounts(params(json!({"source_account_id": GBP, "target_account_id": "d98a0bdc-5b4d-e2db-0040-0015908f444f", "amount": "20", "currency": "GBP"})))
            .await
            .unwrap(),
    );
    assert_eq!(
        fake.last("POST", "transfer").body.unwrap()["amount"],
        json!(20)
    );
    let t = error_text(
        &s.transfer_between_accounts(params(json!({"source_account_id": GBP, "target_account_id": GBP, "amount": "20", "currency": "GBP"})))
            .await
            .unwrap(),
    );
    assert!(t.contains("the same"), "{t}");
    json(
        &s.exchange_currency(params(json!({"from_account_id": GBP, "from_currency": "GBP", "to_account_id": EUR, "to_currency": "EUR", "sell_amount": "10"})))
            .await
            .unwrap(),
    );
    let body = fake.last("POST", "exchange").body.unwrap();
    assert_eq!(body["from"]["amount"], json!(10));
    assert!(body["to"].get("amount").is_none());
}

#[tokio::test]
async fn counterparties_are_saved_listed_and_deleted() {
    let fake = FakeRevolut::start().await;
    fake.on(
        "POST",
        "counterparty",
        Canned::ok(json!({"id": CP, "name": "Acme Ltd", "state": "created", "accounts": [{"id": "acc", "account_no": "12345678"}]})),
    );
    fake.on(
        "GET",
        "counterparties",
        Canned::ok(json!([{"id": CP, "name": "Acme Ltd"}])),
    );
    fake.on(
        "DELETE",
        &format!("counterparty/{CP}"),
        Canned::status(204, serde_json::Value::Null),
    );
    let s = server(&fake);
    let v = json(
        &s.create_counterparty(params(json!({"company_name": "Acme Ltd", "account_no": "12345678", "sort_code": "12-34-56", "bank_country": "GB", "currency": "GBP"})))
            .await
            .unwrap(),
    );
    assert_eq!(v["id"], CP);
    json(
        &s.list_counterparties(params(json!({"name": "Acme"})))
            .await
            .unwrap(),
    );
    let l = fake.last("GET", "counterparties");
    assert_eq!(l.param("name"), Some("Acme"));
    assert_eq!(l.param("limit"), Some("100"));
    let v = json(
        &s.delete_counterparty(params(json!({"counterparty_id": CP})))
            .await
            .unwrap(),
    );
    assert_eq!(v, json!({"ok": true}));
    assert_eq!(fake.lock().refused_signatures, 0);
}

#[tokio::test]
async fn cards_are_frozen_and_never_show_secrets() {
    let fake = FakeRevolut::start().await;
    fake.on(
        "GET",
        "cards",
        Canned::ok(json!([{"id": "c1", "last_digits": "1234", "state": "active", "pan": "4111111111111234", "cvv": "999"}])),
    );
    fake.on(
        "POST",
        "cards/c1/freeze",
        Canned::status(204, serde_json::Value::Null),
    );
    let s = server(&fake);
    let v = json(&s.list_cards(params(json!({}))).await.unwrap());
    assert!(v["items"][0].get("pan").is_none() && v["items"][0].get("cvv").is_none());
    let v = json(
        &s.freeze_card(params(json!({"card_id": "c1"})))
            .await
            .unwrap(),
    );
    assert_eq!(v["state"], "frozen");
}

#[tokio::test]
async fn api_get_reaches_other_endpoints_but_not_auth() {
    let fake = FakeRevolut::start().await;
    fake.on(
        "GET",
        "transfer-reasons",
        Canned::ok(json!([{"code": "goods", "country": "IN"}])),
    );
    let s = server(&fake);
    let v = json(
        &s.api_get(params(
            json!({"path": "/api/1.0/transfer-reasons", "query": {"country": "IN"}}),
        ))
        .await
        .unwrap(),
    );
    assert_eq!(v["count"], 1);
    assert_eq!(
        fake.last("GET", "transfer-reasons").param("country"),
        Some("IN")
    );
    for bad in [
        "auth/token",
        "https://evil.example.com/api/1.0/accounts",
        "accounts/../auth",
        "accounts?x=1",
    ] {
        let t = error_text(&s.api_get(params(json!({"path": bad}))).await.unwrap());
        assert!(t.starts_with("validation:"), "{bad}: {t}");
    }
    assert_eq!(fake.lock().refused_signatures, 0);
}

fn tx(id: usize, at: &str, amount: f64) -> serde_json::Value {
    json!({"id": format!("t{id}"), "type": "card_payment", "state": "completed", "created_at": at, "completed_at": at,
        "merchant": {"name": if id.is_multiple_of(2) { "Even Ltd" } else { "Odd Ltd" }},
        "legs": [{"account_id": GBP, "amount": amount, "fee": 0, "currency": "GBP", "description": format!("Shop {id}")}]})
}

#[tokio::test]
async fn summaries_page_through_every_transaction_once() {
    let fake = FakeRevolut::start().await;
    // A full first page whose oldest instant is shared with the second page's first item.
    let mut first: Vec<serde_json::Value> = (0..1000)
        .map(|i| tx(i, &format!("2026-10-{:02}T12:00:00Z", 30 - (i / 40)), -1.0))
        .collect();
    first[999] = tx(999, "2026-10-05T12:00:00.000Z", -1.0);
    let second = vec![
        tx(999, "2026-10-05T12:00:00.000Z", -1.0),
        tx(1000, "2026-10-05T12:00:00.000Z", -2.5),
        tx(1001, "2026-10-02T09:00:00Z", 100.0),
    ];
    fake.on_seq(
        "GET",
        "transactions",
        vec![Canned::ok(json!(first)), Canned::ok(json!(second))],
    );
    let v = json(
        &server(&fake)
            .summarise_transactions(params(json!({"from": "2026-10-01", "to": "2026-11-01"})))
            .await
            .unwrap(),
    );
    assert_eq!(v["transactions"], 1002, "the repeated t999 is counted once");
    assert_eq!(v["by_currency"]["GBP"]["money_out"], "1002.50");
    assert_eq!(v["by_currency"]["GBP"]["money_in"], "100.00");
    assert_eq!(v["top_merchants"][0]["merchant"], "Even Ltd");
    let log = fake.log();
    assert_eq!(log.len(), 2);
    assert_eq!(log[1].param("to"), Some("2026-10-05T12:00:00.001Z"));
    assert_eq!(log[1].param("from"), Some("2026-10-01"));
    assert!(v.get("truncated").is_none());
}

#[tokio::test]
async fn statements_come_as_csv_for_one_account() {
    let fake = FakeRevolut::start().await;
    fake.on(
        "GET",
        "transactions",
        Canned::ok(json!([
            {"id": "b", "type": "transfer", "state": "completed", "created_at": "2026-10-03T10:00:00Z", "completed_at": "2026-10-03T10:00:00Z",
             "legs": [{"account_id": GBP, "amount": -20, "fee": 0.3, "currency": "GBP", "description": "To Acme", "balance": 79.7}]},
            {"id": "a", "type": "topup", "state": "completed", "created_at": "2026-10-01T10:00:00Z", "completed_at": "2026-10-01T10:00:00Z",
             "legs": [{"account_id": GBP, "amount": 100, "currency": "GBP", "description": "Top up", "balance": 100}]}
        ])),
    );
    let r = server(&fake)
        .export_statement(params(json!({"account_id": GBP, "from": "2026-10-01", "to": "2026-10-31", "format": "FreeAgent"})))
        .await
        .unwrap();
    let summary: serde_json::Value = serde_json::from_str(&support::tools::text(&r)).unwrap();
    assert_eq!(summary["lines"], 2);
    assert_eq!(summary["net_change"], "79.70");
    assert_eq!(summary["closing_balance"], "79.70");
    let csv = r.content[1].as_text().unwrap().text.clone();
    assert_eq!(
        csv,
        "01/10/2026,100.00,Top up\n03/10/2026,-20.00,To Acme\n03/10/2026,-0.30,Fee: To Acme\n"
    );
    let l = fake.last("GET", "transactions");
    assert_eq!(l.param("account"), Some(GBP));
    assert_eq!(
        l.param("to"),
        Some("2026-11-01"),
        "the last day is included"
    );
    let t = error_text(
        &server(&fake)
            .export_statement(params(
                json!({"account_id": GBP, "from": "2026-10-31", "to": "2026-10-01"}),
            ))
            .await
            .unwrap(),
    );
    assert!(t.contains("from must not be after to"), "{t}");
}

#[tokio::test]
async fn quotes_requirements_and_reasons() {
    let fake = FakeRevolut::start().await;
    fake.on(
        "POST",
        "pay/indicative-quote",
        Canned::ok(json!({"amount": {"amount": 100, "currency": "USD"}, "fee": {"amount": 1.5, "currency": "GBP"},
            "estimated_total": {"amount": 80.5, "currency": "GBP"}, "estimated_exchange_rate": "1.27", "warnings": ["volatile_fx_rate"]})),
    );
    fake.on(
        "GET",
        "counterparties/fields",
        Canned::ok(json!({"fields": [{"name": "routing_number", "required": true}, {"name": "route", "required": true, "options": [{"value": "ach"}]}]})),
    );
    fake.on("GET", "transfer-reasons", Canned::ok(json!([{"country": "IN", "currency": "INR", "code": "services", "description": "Services"}])));
    let s = server(&fake);
    let v = json(
        &s.get_payment_quote(params(json!({"account_id": GBP, "counterparty_id": CP, "amount": "100", "currency": "usd", "charge_bearer": "shared"})))
            .await
            .unwrap(),
    );
    assert_eq!(v["warnings"], json!(["volatile_fx_rate"]));
    let body = fake.last("POST", "pay/indicative-quote").body.unwrap();
    assert_eq!(
        body,
        json!({"account_id": GBP, "receiver": {"counterparty_id": CP}, "amount": 100, "currency": "USD", "charge_bearer": "shared"})
    );
    let v = json(
        &s.get_counterparty_requirements(params(json!({"country": "us", "currency": "USD", "recipient_type": "Business", "route": "ACH"})))
            .await
            .unwrap(),
    );
    assert_eq!(v["fields"][0]["name"], "routing_number");
    let l = fake.last("GET", "counterparties/fields");
    assert_eq!(l.param("country"), Some("US"));
    assert_eq!(l.param("recipient_type"), Some("business"));
    assert_eq!(l.param("route"), Some("ach"));
    assert_eq!(
        json(
            &s.list_transfer_reasons(params(json!({"country": "in"})))
                .await
                .unwrap()
        )["count"],
        1
    );
    assert_eq!(
        json(
            &s.list_transfer_reasons(params(json!({"currency": "EUR"})))
                .await
                .unwrap()
        )["count"],
        0
    );
    assert!(
        error_text(&s.list_transfer_reasons(params(json!({}))).await.unwrap())
            .contains("give a country")
    );
    assert_eq!(fake.lock().refused_signatures, 0);
}

#[tokio::test]
async fn names_are_checked_for_uk_and_eur_accounts() {
    let fake = FakeRevolut::start().await;
    fake.on("POST", "account-name-validation", Canned::ok(json!({"result_code": "close_match", "company_name": "Acme Limited", "name_validation_id": "nv-1"})));
    let s = server(&fake);
    let v = json(
        &s.validate_account_name(params(json!({"iban": "FR76 3000 6000 0112 3456 7890 189", "recipient_country": "fr", "company_name": "Acme"})))
            .await
            .unwrap(),
    );
    assert_eq!(v["result_code"], "close_match");
    assert_eq!(
        fake.last("POST", "account-name-validation").body.unwrap(),
        json!({"iban": "FR7630006000011234567890189", "recipient_country": "FR", "recipient_currency": "EUR", "company_name": "Acme"})
    );
    json(
        &s.validate_account_name(params(json!({"account_no": "12345678", "sort_code": "12-34-56", "first_name": "Ann", "last_name": "Lee"})))
            .await
            .unwrap(),
    );
    assert_eq!(
        fake.last("POST", "account-name-validation").body.unwrap(),
        json!({"account_no": "12345678", "sort_code": "123456", "individual_name": {"first_name": "Ann", "last_name": "Lee"}})
    );
    for bad in [
        json!({"company_name": "Acme"}),
        json!({"iban": "FR7630006000011234567890189", "company_name": "Acme"}),
        json!({"account_no": "12345678", "sort_code": "123456", "iban": "FR7630006000011234567890189", "company_name": "Acme"}),
    ] {
        let t = error_text(&s.validate_account_name(params(bad.clone())).await.unwrap());
        assert!(t.starts_with("validation:"), "{bad}: {t}");
    }
}

#[tokio::test]
async fn counterparties_abroad_take_more_fields() {
    let fake = FakeRevolut::start().await;
    fake.on("POST", "counterparty", Canned::ok(json!({"id": CP})));
    let s = server(&fake);
    json(
        &s.create_counterparty(params(json!({"company_name": "Acme Inc", "account_no": "000123456789", "routing_number": "021000021",
            "bank_country": "US", "currency": "USD", "more_fields": {"route": "ach", "account_type": "checking"}})))
            .await
            .unwrap(),
    );
    let body = fake.last("POST", "counterparty").body.unwrap();
    assert_eq!(body["route"], "ach");
    assert_eq!(body["account_type"], "checking");
    let t = error_text(
        &s.create_counterparty(params(
            json!({"revtag": "x", "more_fields": {"request_id": "x"}}),
        ))
        .await
        .unwrap(),
    );
    assert!(t.contains("more_fields may only hold"), "{t}");
}

#[tokio::test]
async fn cards_are_issued_changed_locked_and_terminated() {
    let fake = FakeRevolut::start().await;
    fake.on(
        "POST",
        "cards",
        Canned::ok(json!({"id": "c9", "virtual": true, "state": "active", "label": "Ads"})),
    );
    fake.on(
        "PATCH",
        "cards/c9",
        Canned::ok(json!({"id": "c9", "label": "Ads 2"})),
    );
    for path in ["cards/c9/lock", "cards/c9/unlock"] {
        fake.on("POST", path, Canned::status(204, serde_json::Value::Null));
    }
    fake.on(
        "DELETE",
        "cards/c9",
        Canned::status(204, serde_json::Value::Null),
    );
    let s = server(&fake);
    let v = json(
        &s.create_virtual_card(params(json!({"label": "Ads", "contact_ids": [GBP], "spending_limits": {"month": {"amount": "500", "currency": "GBP"}}, "account_ids": [GBP]})))
            .await
            .unwrap(),
    );
    assert_eq!(v["id"], "c9");
    let body = fake.last("POST", "cards").body.unwrap();
    assert_eq!(body["virtual"], true);
    assert_eq!(body["accounts"], json!([GBP]));
    assert_eq!(body["request_id"].as_str().unwrap().len(), 36);
    json(
        &s.update_card(params(json!({"card_id": "c9", "label": "Ads 2"})))
            .await
            .unwrap(),
    );
    assert_eq!(
        fake.last("PATCH", "cards/c9").body.unwrap(),
        json!({"label": "Ads 2"})
    );
    assert_eq!(
        json(&s.lock_card(params(json!({"card_id": "c9"}))).await.unwrap())["state"],
        "locked"
    );
    json(
        &s.unlock_card(params(json!({"card_id": "c9"})))
            .await
            .unwrap(),
    );
    assert_eq!(
        json(
            &s.terminate_card(params(json!({"card_id": "c9"})))
                .await
                .unwrap()
        )["state"],
        "terminated"
    );
    assert_eq!(fake.lock().refused_signatures, 0);
}

#[tokio::test]
async fn the_team_is_managed() {
    let fake = FakeRevolut::start().await;
    let m = "7f1e2d3c-0000-4000-8000-000000000001";
    fake.on(
        "GET",
        "roles",
        Canned::ok(json!([{"id": "Owner", "name": "Owner"}, {"id": "r-1", "name": "Accountant"}])),
    );
    fake.on(
        "POST",
        "team-members",
        Canned::status(
            201,
            json!({"id": m, "email": "ann@example.com", "role_id": "r-1"}),
        ),
    );
    fake.on(
        "PUT",
        &format!("team-members/{m}/role"),
        Canned::status(204, serde_json::Value::Null),
    );
    for a in ["suspend", "unsuspend"] {
        fake.on(
            "POST",
            &format!("team-members/{m}/{a}"),
            Canned::status(204, serde_json::Value::Null),
        );
    }
    fake.on(
        "DELETE",
        &format!("team-members/{m}"),
        Canned::status(204, serde_json::Value::Null),
    );
    fake.on(
        "GET",
        &format!("team-members/{m}"),
        Canned::ok(json!({"id": m, "email": "ann@example.com", "state": "active"})),
    );
    let s = server(&fake);
    assert_eq!(
        json(&s.list_roles(params(json!({}))).await.unwrap())["count"],
        2
    );
    json(
        &s.invite_team_member(params(
            json!({"email": "ann@example.com", "role_id": "r-1"}),
        ))
        .await
        .unwrap(),
    );
    assert_eq!(
        fake.last("POST", "team-members").body.unwrap(),
        json!({"email": "ann@example.com", "role_id": "r-1"})
    );
    json(
        &s.change_team_member_role(params(json!({"team_member_id": m, "role_id": "Owner"})))
            .await
            .unwrap(),
    );
    assert_eq!(
        fake.last("PUT", &format!("team-members/{m}/role"))
            .body
            .unwrap(),
        json!({"role_id": "Owner"})
    );
    json(
        &s.suspend_team_member(params(json!({"team_member_id": m})))
            .await
            .unwrap(),
    );
    json(
        &s.unsuspend_team_member(params(json!({"team_member_id": m})))
            .await
            .unwrap(),
    );
    assert_eq!(
        json(
            &s.get_team_member(params(json!({"team_member_id": m})))
                .await
                .unwrap()
        )["state"],
        "active"
    );
    assert_eq!(
        json(
            &s.remove_team_member(params(json!({"team_member_id": m})))
                .await
                .unwrap()
        )["state"],
        "removed"
    );
    let t = error_text(
        &s.invite_team_member(params(json!({"email": "not an email", "role_id": "r-1"})))
            .await
            .unwrap(),
    );
    assert!(t.starts_with("validation:"), "{t}");
    assert_eq!(fake.lock().refused_signatures, 0);
}
