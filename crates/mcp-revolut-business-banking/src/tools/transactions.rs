//! Transactions: statements across accounts, and one transaction.

use super::{
    BankingServer, Query, choice, count_arg, id_arg, json_result, opt_id, push, run, when_arg,
};
use crate::error::RvError;
use crate::view;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::Value;

/// Revolut returns at most this many transactions at once.
pub const MAX_TRANSACTIONS: u32 = 1000;
const DEFAULT_TRANSACTIONS: u32 = 50;

pub const TRANSACTION_TYPES: &[&str] = &[
    "atm",
    "card_payment",
    "card_refund",
    "card_chargeback",
    "card_credit",
    "charge",
    "charge_refund",
    "exchange",
    "transfer",
    "loan",
    "fee",
    "refund",
    "topup",
    "topup_return",
    "tax",
    "tax_refund",
    "rev_payment",
];

pub const TRANSACTION_STATES: &[&str] = &[
    "created",
    "pending",
    "completed",
    "declined",
    "failed",
    "reverted",
];

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListTransactionsParams {
    /// Only transactions created on or after this date or date-time (YYYY-MM-DD or ISO 8601).
    pub from: Option<String>,
    /// Only transactions created before this date or date-time. For older pages, pass the
    /// created_at of the oldest transaction from the last call.
    pub to: Option<String>,
    /// Only transactions on this account (id from list_accounts).
    pub account_id: Option<String>,
    /// Only this type: atm, card_payment, card_refund, card_chargeback, card_credit, exchange,
    /// transfer, loan, fee, refund, topup, topup_return, tax or tax_refund.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Only transactions in these states: created, pending, completed, declined, failed or
    /// reverted.
    pub states: Option<Vec<String>>,
    /// Only the transaction sent with this request_id (needs from, at most 14 days before to):
    /// how to check whether a payment that timed out went through.
    pub request_id: Option<String>,
    /// How many, newest first: 1 to 1000. Default 50.
    pub count: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct TransactionParams {
    /// The transaction id, from list_transactions or from the payment that made it.
    pub transaction_id: String,
}

#[tool_router(router = transactions_router, vis = "pub(crate)")]
impl BankingServer {
    #[tool(
        description = "Transactions across the business's accounts, newest first: id, type, state (created, pending, completed, declined, failed, reverted), created and completed times, reference, merchant or counterparty, and legs (one per account touched, each with amount, fee, currency, balance after and description). Money out is negative. Filter by dates, account and type; for older ones pass the oldest created_at returned as to.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn list_transactions(
        &self,
        Parameters(p): Parameters<ListTransactionsParams>,
    ) -> Result<CallToolResult, McpError> {
        run("list_transactions", async {
            let c = self.caller();
            let count = count_arg(p.count, DEFAULT_TRANSACTIONS, MAX_TRANSACTIONS)?;
            let from = when_arg("from", p.from.as_deref())?;
            let to = when_arg("to", p.to.as_deref())?;
            if let (Some(f), Some(t)) = (&from, &to)
                && f.get(..10) > t.get(..10)
            {
                return Err(RvError::invalid("from must not be after to"));
            }
            let mut q: Query = vec![("count", count.to_string())];
            push(&mut q, "from", from);
            push(&mut q, "to", to);
            push(
                &mut q,
                "account",
                opt_id("account_id", p.account_id.as_deref())?,
            );
            push(
                &mut q,
                "type",
                choice("type", p.kind.as_deref(), TRANSACTION_TYPES)?.map(str::to_string),
            );
            for st in p.states.iter().flatten() {
                if let Some(v) = choice("states", Some(st), TRANSACTION_STATES)? {
                    q.push(("state", v.to_string()));
                }
            }
            if let Some(r) = p.request_id.as_deref().filter(|r| !r.trim().is_empty()) {
                if !q.iter().any(|(k, _)| *k == "from") {
                    return Err(RvError::invalid(
                        "request_id needs from: give a from date at most 14 days before to",
                    ));
                }
                q.push(("request_id", super::request_id(Some(r))?));
            }
            let items = view::items(
                c.get("transactions", &["transactions"], &q).await?,
                "transactions",
            );
            let more = (items.len() as u32 >= count).then(|| {
                let oldest = items
                    .last()
                    .and_then(|t| t.get("created_at"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                format!("there may be older transactions: call again with to {oldest}")
            });
            Ok(json_result(&view::list(items, more)))
        })
        .await
    }

    #[tool(
        description = "One transaction in full, by id: type, state and reason, times, reference, merchant, card, and each leg with its counterparty, amount, fee and balance after.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn get_transaction(
        &self,
        Parameters(p): Parameters<TransactionParams>,
    ) -> Result<CallToolResult, McpError> {
        run("get_transaction", async {
            let c = self.caller();
            let id = id_arg("transaction_id", &p.transaction_id)?;
            Ok(json_result(
                &c.one("transaction", &["transaction", &id], &[]).await?,
            ))
        })
        .await
    }
}
