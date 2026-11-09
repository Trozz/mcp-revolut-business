//! Payments that move money at once: to a counterparty, between the business's own accounts, and
//! currency exchange. Present only when the server allows payments (`REVOLUT_ALLOW_PAYMENTS`).

use super::drafts::receiver;
use super::{
    BankingServer, amount_arg, choice, currency_arg, fields, id_arg, json_result, opt_amount,
    opt_s, payments_allowed, request_id, run, s, text_limited,
};
use crate::error::RvError;
use reqwest::Method;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct PayParams {
    /// The account to pay from (id from list_accounts).
    pub account_id: String,
    /// The counterparty to pay (id from list_counterparties).
    pub counterparty_id: String,
    /// Which of the counterparty's accounts to pay, when it has more than one.
    pub counterparty_account_id: Option<String>,
    /// The amount, e.g. 250.00.
    pub amount: String,
    /// The currency, e.g. GBP.
    pub currency: String,
    /// The reference the recipient sees.
    pub reference: Option<String>,
    /// Who pays the fees for international payments: shared, debtor or prefer_debtor.
    pub charge_bearer: Option<String>,
    /// A transfer reason code, where the destination needs one (list_transfer_reasons).
    pub transfer_reason_code: Option<String>,
    /// The name_validation_id validate_account_name returned for this recipient, to show the
    /// name was checked.
    pub name_validation_id: Option<String>,
    /// To retry a payment whose answer was lost, the request_id it returned. Revolut makes a
    /// payment once per request_id, so a retry cannot pay twice. Leave out for a new payment.
    pub request_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct TransferParams {
    /// The account to move money from.
    pub source_account_id: String,
    /// The account to move it to, in the same currency.
    pub target_account_id: String,
    /// The amount, e.g. 250.00.
    pub amount: String,
    /// The currency of both accounts.
    pub currency: String,
    /// A reference for the business's records.
    pub reference: Option<String>,
    /// To retry a transfer whose answer was lost, the request_id it returned.
    pub request_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ExchangeParams {
    /// The account to sell from.
    pub from_account_id: String,
    /// Its currency, e.g. GBP.
    pub from_currency: String,
    /// The account to buy into.
    pub to_account_id: String,
    /// Its currency, e.g. EUR.
    pub to_currency: String,
    /// How much to sell. Give this or buy_amount, not both.
    pub sell_amount: Option<String>,
    /// How much to buy. Give this or sell_amount, not both.
    pub buy_amount: Option<String>,
    /// A reference for the business's records.
    pub reference: Option<String>,
    /// To retry an exchange whose answer was lost, the request_id it returned.
    pub request_id: Option<String>,
}

fn reference(v: Option<&str>) -> Result<Option<Value>, RvError> {
    Ok(opt_s(text_limited("reference", v, 100)?))
}

/// The payment's answer, with the request_id to retry or look it up by.
fn answered(mut out: Value, request_id: String) -> Value {
    if let Value::Object(m) = &mut out {
        m.insert("request_id".into(), Value::from(request_id));
    }
    out
}

pub(crate) fn pay_body(p: &PayParams, rid: &str) -> Result<Value, RvError> {
    Ok(Value::Object(fields(vec![
        ("request_id", s(rid)),
        ("account_id", s(id_arg("account_id", &p.account_id)?)),
        (
            "receiver",
            Some(receiver(
                &p.counterparty_id,
                p.counterparty_account_id.as_deref(),
            )?),
        ),
        ("amount", Some(amount_arg("amount", &p.amount)?)),
        ("currency", s(currency_arg("currency", &p.currency)?)),
        ("reference", reference(p.reference.as_deref())?),
        (
            "charge_bearer",
            choice(
                "charge_bearer",
                p.charge_bearer.as_deref(),
                &["shared", "debtor", "prefer_debtor"],
            )?
            .and_then(s),
        ),
        (
            "transfer_reason_code",
            super::quotes::reason_code(p.transfer_reason_code.as_deref())?,
        ),
        (
            "name_validation_id",
            opt_s(super::opt_id(
                "name_validation_id",
                p.name_validation_id.as_deref(),
            )?),
        ),
    ])))
}

pub(crate) fn exchange_body(p: &ExchangeParams, rid: &str) -> Result<Value, RvError> {
    let sell = opt_amount("sell_amount", p.sell_amount.as_deref())?;
    let buy = opt_amount("buy_amount", p.buy_amount.as_deref())?;
    if sell.is_some() == buy.is_some() {
        return Err(RvError::invalid("give sell_amount or buy_amount, not both"));
    }
    let from_currency = currency_arg("from_currency", &p.from_currency)?;
    let to_currency = currency_arg("to_currency", &p.to_currency)?;
    if from_currency == to_currency {
        return Err(RvError::invalid(
            "the currencies are the same: use transfer_between_accounts",
        ));
    }
    Ok(Value::Object(fields(vec![
        ("request_id", s(rid)),
        (
            "from",
            Some(Value::Object(fields(vec![
                (
                    "account_id",
                    s(id_arg("from_account_id", &p.from_account_id)?),
                ),
                ("currency", s(from_currency)),
                ("amount", sell),
            ]))),
        ),
        (
            "to",
            Some(Value::Object(fields(vec![
                ("account_id", s(id_arg("to_account_id", &p.to_account_id)?)),
                ("currency", s(to_currency)),
                ("amount", buy),
            ]))),
        ),
        ("reference", reference(p.reference.as_deref())?),
    ])))
}

#[tool_router(router = payments_router, vis = "pub(crate)")]
impl BankingServer {
    #[tool(
        description = "Pays a counterparty now, from one of the business's accounts. Money leaves at once and cannot be called back: prefer create_payment_draft unless the person asks to pay directly. Before calling, show the person the account, recipient, amount, currency and reference, and get their explicit agreement. Answers with the transaction id, state and request_id; if the call fails without an answer, check list_transactions with that request_id before trying again, or retry with the same request_id.",
        annotations(destructive_hint = true, open_world_hint = true)
    )]
    pub async fn create_payment(
        &self,
        Parameters(p): Parameters<PayParams>,
    ) -> Result<CallToolResult, McpError> {
        run("create_payment", async {
            payments_allowed(self.allows_payments())?;
            let c = self.caller();
            let rid = request_id(p.request_id.as_deref())?;
            let body = pay_body(&p, &rid)?;
            let out = c.write(Method::POST, "pay", &["pay"], Some(&body)).await?;
            Ok(json_result(&answered(out, rid)))
        })
        .await
    }

    #[tool(
        description = "Moves money now between two of the business's own accounts in the same currency. Show the person the accounts and amount and get their agreement first.",
        annotations(destructive_hint = true, open_world_hint = true)
    )]
    pub async fn transfer_between_accounts(
        &self,
        Parameters(p): Parameters<TransferParams>,
    ) -> Result<CallToolResult, McpError> {
        run("transfer_between_accounts", async {
            payments_allowed(self.allows_payments())?;
            let c = self.caller();
            let source = id_arg("source_account_id", &p.source_account_id)?;
            let target = id_arg("target_account_id", &p.target_account_id)?;
            if source == target {
                return Err(RvError::invalid(
                    "the source and target accounts are the same",
                ));
            }
            let rid = request_id(p.request_id.as_deref())?;
            let body = Value::Object(fields(vec![
                ("request_id", s(&rid)),
                ("source_account_id", s(source)),
                ("target_account_id", s(target)),
                ("amount", Some(amount_arg("amount", &p.amount)?)),
                ("currency", s(currency_arg("currency", &p.currency)?)),
                ("reference", reference(p.reference.as_deref())?),
            ]));
            let out = c
                .write(Method::POST, "transfer", &["transfer"], Some(&body))
                .await?;
            Ok(json_result(&answered(out, rid)))
        })
        .await
    }

    #[tool(
        description = "Exchanges currency now between two of the business's accounts, at Revolut's current rate: give the amount to sell or the amount to buy. Check the rate with get_exchange_rate, show the person what they will sell and get, and get their agreement first.",
        annotations(destructive_hint = true, open_world_hint = true)
    )]
    pub async fn exchange_currency(
        &self,
        Parameters(p): Parameters<ExchangeParams>,
    ) -> Result<CallToolResult, McpError> {
        run("exchange_currency", async {
            payments_allowed(self.allows_payments())?;
            let c = self.caller();
            let rid = request_id(p.request_id.as_deref())?;
            let body = exchange_body(&p, &rid)?;
            let out = c
                .write(Method::POST, "exchange", &["exchange"], Some(&body))
                .await?;
            Ok(json_result(&answered(out, rid)))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const A: &str = "1714b6ee-ca1a-e2cf-0040-00d3af294457";
    const B: &str = "84d79450-bd2c-e211-0040-008cab314b30";

    #[test]
    fn a_payment_body() {
        let p: PayParams = serde_json::from_value(json!({"account_id": A, "counterparty_id": B, "amount": "10", "currency": "GBP", "reference": "Rent"})).unwrap();
        assert_eq!(
            pay_body(&p, "r1").unwrap(),
            json!({"request_id": "r1", "account_id": A, "receiver": {"counterparty_id": B}, "amount": 10, "currency": "GBP", "reference": "Rent"})
        );
    }

    #[test]
    fn an_exchange_names_one_side() {
        let p: ExchangeParams = serde_json::from_value(json!({"from_account_id": A, "from_currency": "GBP", "to_account_id": B, "to_currency": "EUR", "buy_amount": "100"})).unwrap();
        assert_eq!(
            exchange_body(&p, "r").unwrap(),
            json!({"request_id": "r", "from": {"account_id": A, "currency": "GBP"}, "to": {"account_id": B, "currency": "EUR", "amount": 100}})
        );
        for bad in [
            json!({"from_account_id": A, "from_currency": "GBP", "to_account_id": B, "to_currency": "EUR"}),
            json!({"from_account_id": A, "from_currency": "GBP", "to_account_id": B, "to_currency": "EUR", "sell_amount": "1", "buy_amount": "1"}),
            json!({"from_account_id": A, "from_currency": "GBP", "to_account_id": B, "to_currency": "gbp", "sell_amount": "1"}),
        ] {
            let p: ExchangeParams = serde_json::from_value(bad.clone()).unwrap();
            assert!(exchange_body(&p, "r").is_err(), "{bad}");
        }
    }
}
