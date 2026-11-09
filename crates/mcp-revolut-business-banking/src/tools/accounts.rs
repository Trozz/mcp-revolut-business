//! Accounts, their bank details, exchange rates, and a snapshot of where the business stands.

use super::{BankingServer, Caller, amount_arg, currency_arg, id_arg, json_result, run};
use crate::error::{self, RvError};
use crate::view::{self, minor, money};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct NoParams {}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AccountParams {
    /// The account id, from list_accounts.
    pub account_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RateParams {
    /// The currency to sell, e.g. GBP.
    pub from: String,
    /// The currency to buy, e.g. EUR.
    pub to: String,
    /// How much of `from` to exchange. Default 1.
    pub amount: Option<String>,
}

/// Most recent transactions in the snapshot.
const SNAPSHOT_TRANSACTIONS: u32 = 10;

/// Balances of active accounts summed by currency, in minor units.
fn totals(accounts: &[Value]) -> Value {
    let mut by: BTreeMap<String, i64> = BTreeMap::new();
    for a in accounts {
        if a.get("state")
            .and_then(Value::as_str)
            .is_some_and(|s| s != "active")
        {
            continue;
        }
        if let (Some(c), Some(b)) = (
            a.get("currency").and_then(Value::as_str),
            minor(a.get("balance")),
        ) {
            *by.entry(c.to_string()).or_default() += b;
        }
    }
    Value::Object(
        by.into_iter()
            .map(|(c, m)| (c, Value::from(money(m))))
            .collect(),
    )
}

/// A transaction in a line: when, what, state, and each leg's amount and description.
pub(crate) fn brief_transaction(t: &Value) -> Value {
    let legs: Vec<Value> = t
        .get("legs")
        .and_then(Value::as_array)
        .map(|legs| {
            legs.iter()
                .map(|l| {
                    view::resource(json!({
                        "account_id": l.get("account_id"),
                        "amount": l.get("amount"),
                        "currency": l.get("currency"),
                        "description": l.get("description"),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    view::resource(json!({
        "id": t.get("id"),
        "type": t.get("type"),
        "state": t.get("state"),
        "created_at": t.get("created_at"),
        "reference": t.get("reference"),
        "merchant": t.get("merchant").and_then(|m| m.get("name")),
        "legs": legs,
    }))
}

impl BankingServer {
    async fn snapshot(&self, c: &Caller) -> error::Result<Value> {
        let mut out = Map::new();
        let mut unavailable = Map::new();
        let accounts = match c.get("accounts", &["accounts"], &[]).await {
            Ok(a) => view::items(a, "accounts"),
            Err(e @ (RvError::Auth(_) | RvError::RateLimited { .. })) => return Err(e),
            Err(e) => {
                unavailable.insert("accounts".into(), Value::from(format!("{}: {e}", e.kind())));
                Vec::new()
            }
        };
        out.insert("balances_by_currency".into(), totals(&accounts));
        let rows: Vec<Value> = accounts
            .iter()
            .filter(|a| a.get("state").and_then(Value::as_str) != Some("inactive"))
            .map(|a| {
                view::resource(json!({
                    "id": a.get("id"),
                    "name": a.get("name"),
                    "currency": a.get("currency"),
                    "balance": a.get("balance"),
                    "state": a.get("state"),
                }))
            })
            .collect();
        out.insert("accounts".into(), Value::Array(rows));
        let q = vec![("count", SNAPSHOT_TRANSACTIONS.to_string())];
        match c.get("transactions", &["transactions"], &q).await {
            Ok(t) => {
                let items = view::items(t, "transactions");
                let pending = items
                    .iter()
                    .filter(|t| t.get("state").and_then(Value::as_str) == Some("pending"))
                    .count();
                out.insert(
                    "recent_transactions".into(),
                    Value::Array(items.iter().map(brief_transaction).collect()),
                );
                out.insert("pending_in_recent".into(), Value::from(pending));
            }
            Err(e) => {
                unavailable.insert(
                    "recent_transactions".into(),
                    Value::from(format!("{}: {e}", e.kind())),
                );
            }
        }
        match c.get("payment_drafts", &["payment-drafts"], &[]).await {
            Ok(d) => {
                let drafts = view::items(d, "payment_orders");
                out.insert("payment_drafts_awaiting".into(), Value::from(drafts.len()));
            }
            Err(e) => {
                unavailable.insert(
                    "payment_drafts".into(),
                    Value::from(format!("{}: {e}", e.kind())),
                );
            }
        }
        if !unavailable.is_empty() {
            out.insert("unavailable".into(), Value::Object(unavailable));
        }
        Ok(Value::Object(out))
    }
}

#[tool_router(router = accounts_router, vis = "pub(crate)")]
impl BankingServer {
    #[tool(
        description = "The business's Revolut accounts: id, name, currency, balance, state (active or inactive), whether public, and when created and last updated. Each currency is its own account.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn list_accounts(
        &self,
        Parameters(_p): Parameters<NoParams>,
    ) -> Result<CallToolResult, McpError> {
        run("list_accounts", async {
            let c = self.caller();
            let items = view::items(c.get("accounts", &["accounts"], &[]).await?, "accounts");
            Ok(json_result(&view::list(items, None)))
        })
        .await
    }

    #[tool(
        description = "One account by id: name, currency, balance and state.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn get_account(
        &self,
        Parameters(p): Parameters<AccountParams>,
    ) -> Result<CallToolResult, McpError> {
        run("get_account", async {
            let c = self.caller();
            let id = id_arg("account_id", &p.account_id)?;
            Ok(json_result(
                &c.one("account", &["accounts", &id], &[]).await?,
            ))
        })
        .await
    }

    #[tool(
        description = "The details others use to pay into an account: beneficiary name and address, account number and sort code, IBAN and BIC, the payment schemes each set supports (such as faster_payments, sepa, swift) and how long they take.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn get_account_bank_details(
        &self,
        Parameters(p): Parameters<AccountParams>,
    ) -> Result<CallToolResult, McpError> {
        run("get_account_bank_details", async {
            let c = self.caller();
            let id = id_arg("account_id", &p.account_id)?;
            let items = view::items(
                c.get("bank_details", &["accounts", &id, "bank-details"], &[])
                    .await?,
                "bank_details",
            );
            Ok(json_result(&view::list(items, None)))
        })
        .await
    }

    #[tool(
        description = "Where the business stands, in one call: balances summed by currency, each active account's balance, the latest transactions (and how many of them are pending), and how many payment drafts await approval. Sections it could not read are listed under unavailable.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn business_snapshot(
        &self,
        Parameters(_p): Parameters<NoParams>,
    ) -> Result<CallToolResult, McpError> {
        run("business_snapshot", async {
            let c = self.caller();
            Ok(json_result(&self.snapshot(&c).await?))
        })
        .await
    }

    #[tool(
        description = "Revolut's current exchange rate between two currencies, and what an amount of `from` would buy in `to`, with the fee. Nothing is exchanged.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn get_exchange_rate(
        &self,
        Parameters(p): Parameters<RateParams>,
    ) -> Result<CallToolResult, McpError> {
        run("get_exchange_rate", async {
            let c = self.caller();
            let from = currency_arg("from", &p.from)?;
            let to = currency_arg("to", &p.to)?;
            if from == to {
                return Err(RvError::invalid("from and to must be different currencies"));
            }
            let mut q = vec![("from", from), ("to", to)];
            if let Some(a) = p.amount.as_deref().filter(|a| !a.trim().is_empty()) {
                q.push(("amount", amount_arg("amount", a)?.to_string()));
            }
            Ok(json_result(&c.one("rate", &["rate"], &q).await?))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_sum_active_accounts_by_currency() {
        let accounts = vec![
            json!({"currency": "GBP", "balance": 100.1, "state": "active"}),
            json!({"currency": "GBP", "balance": 0.2, "state": "active"}),
            json!({"currency": "EUR", "balance": 5, "state": "active"}),
            json!({"currency": "USD", "balance": 9, "state": "inactive"}),
        ];
        assert_eq!(totals(&accounts), json!({"EUR": "5.00", "GBP": "100.30"}));
    }

    #[test]
    fn transactions_are_briefed() {
        let t = json!({"id": "t1", "type": "card_payment", "state": "completed", "created_at": "2026-01-01T10:00:00Z",
            "merchant": {"name": "Coffee Ltd", "city": "London"},
            "legs": [{"leg_id": "l", "account_id": "a", "amount": -3.5, "currency": "GBP", "description": "Coffee", "balance": 10}]});
        let b = brief_transaction(&t);
        assert_eq!(b["merchant"], "Coffee Ltd");
        assert_eq!(
            b["legs"][0],
            json!({"account_id": "a", "amount": -3.5, "currency": "GBP", "description": "Coffee"})
        );
    }
}
