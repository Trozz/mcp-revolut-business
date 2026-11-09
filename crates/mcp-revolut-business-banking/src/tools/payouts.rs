//! Payout links: a link sent to someone so they can choose how to receive money (UK, EEA,
//! Australia and Singapore businesses).

use super::{
    BankingServer, Query, amount_arg, choice, count_arg, currency_arg, fields, id_arg, json_result,
    push, request_id, run, s, text_limited, when_arg,
};
use crate::error::RvError;
use crate::view;
use reqwest::Method;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::Value;

pub const LINK_STATES: &[&str] = &[
    "created",
    "failed",
    "awaiting",
    "active",
    "expired",
    "cancelled",
    "processing",
    "processed",
];

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListLinksParams {
    /// Only links in these states: created, failed, awaiting, active, expired, cancelled,
    /// processing or processed.
    pub states: Option<Vec<String>>,
    /// Only links created before this date-time; for the next page pass the oldest created_at
    /// returned.
    pub created_before: Option<String>,
    /// How many: 1 to 1000. Default 100.
    pub count: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct LinkParams {
    /// The payout link id.
    pub link_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateLinkParams {
    /// Who the money is for, as the link shows it.
    pub counterparty_name: String,
    /// The account to pay from.
    pub account_id: String,
    /// The amount: the equivalent of £1 to £2,500.
    pub amount: String,
    /// The currency, e.g. GBP.
    pub currency: String,
    /// The reference the recipient sees.
    pub reference: String,
    /// How the recipient may be paid: any of revolut, bank_account and card. Default: revolut
    /// and bank_account.
    pub payout_methods: Option<Vec<String>>,
    /// How long the link stays open, in days: 1 to 7. Default 7.
    pub expiry_days: Option<u8>,
    /// Save the recipient as a counterparty once paid.
    pub save_counterparty: Option<bool>,
}

fn link_body(p: CreateLinkParams, rid: &str) -> Result<Value, RvError> {
    let name = text_limited("counterparty_name", Some(&p.counterparty_name), 80)?
        .ok_or_else(|| RvError::invalid("counterparty_name must not be empty"))?;
    let reference = text_limited("reference", Some(&p.reference), 100)?
        .ok_or_else(|| RvError::invalid("reference must not be empty"))?;
    let methods = match p.payout_methods {
        None => None,
        Some(list) => {
            let mut out: Vec<Value> = Vec::new();
            for m in &list {
                if let Some(v) = choice(
                    "payout_methods",
                    Some(m),
                    &["revolut", "bank_account", "card"],
                )? && !out.iter().any(|x| x == v)
                {
                    out.push(Value::from(v));
                }
            }
            (!out.is_empty()).then_some(Value::Array(out))
        }
    };
    let expiry = match p.expiry_days {
        None => None,
        Some(d @ 1..=7) => s(format!("P{d}D")),
        Some(_) => return Err(RvError::invalid("expiry_days must be 1 to 7")),
    };
    Ok(Value::Object(fields(vec![
        ("counterparty_name", s(name)),
        ("request_id", s(rid)),
        ("account_id", s(id_arg("account_id", &p.account_id)?)),
        ("amount", Some(amount_arg("amount", &p.amount)?)),
        ("currency", s(currency_arg("currency", &p.currency)?)),
        ("reference", s(reference)),
        ("payout_methods", methods),
        ("expiry_period", expiry),
        ("save_counterparty", p.save_counterparty.map(Value::from)),
    ])))
}

#[tool_router(router = payouts_router, vis = "pub(crate)")]
impl BankingServer {
    #[tool(
        description = "Payout links, newest first: id, state, recipient name, amount, currency, reference, expiry, and the link url while it is active.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn list_payout_links(
        &self,
        Parameters(p): Parameters<ListLinksParams>,
    ) -> Result<CallToolResult, McpError> {
        run("list_payout_links", async {
            let c = self.caller();
            let count = count_arg(p.count, 100, 1000)?;
            let mut q: Query = vec![("limit", count.to_string())];
            for st in p.states.iter().flatten() {
                if let Some(v) = choice("states", Some(st), LINK_STATES)? {
                    q.push(("state", v.to_string()));
                }
            }
            push(
                &mut q,
                "created_before",
                when_arg("created_before", p.created_before.as_deref())?,
            );
            let items = view::items(
                c.get("payout_links", &["payout-links"], &q).await?,
                "payout_links",
            );
            let more = (items.len() as u32 >= count).then(|| {
                let oldest = items
                    .last()
                    .and_then(|t| t.get("created_at"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                format!("there may be more: call again with created_before {oldest}")
            });
            Ok(json_result(&view::list(items, more)))
        })
        .await
    }

    #[tool(
        description = "One payout link by id, with its state, url (while active) and the transaction once paid.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn get_payout_link(
        &self,
        Parameters(p): Parameters<LinkParams>,
    ) -> Result<CallToolResult, McpError> {
        run("get_payout_link", async {
            let c = self.caller();
            let id = id_arg("link_id", &p.link_id)?;
            Ok(json_result(
                &c.one("payout_link", &["payout-links", &id], &[]).await?,
            ))
        })
        .await
    }

    #[tool(
        description = "Creates a payout link: a link to send to someone so they can collect money from the business by Revolut, bank account or card, without their bank details being known. Money leaves once they claim it. Needs the API key's pay access. Show the person the recipient, amount, currency and reference and get their agreement first. The link url appears once its state is active (get_payout_link).",
        annotations(destructive_hint = true, open_world_hint = true)
    )]
    pub async fn create_payout_link(
        &self,
        Parameters(p): Parameters<CreateLinkParams>,
    ) -> Result<CallToolResult, McpError> {
        run("create_payout_link", async {
            let c = self.caller();
            let rid = request_id(None)?;
            let body = link_body(p, &rid)?;
            Ok(json_result(
                &c.write(
                    Method::POST,
                    "create_payout_link",
                    &["payout-links"],
                    Some(&body),
                )
                .await?,
            ))
        })
        .await
    }

    #[tool(
        description = "Cancels a payout link that has not been claimed. Confirm with the person first.",
        annotations(
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    pub async fn cancel_payout_link(
        &self,
        Parameters(p): Parameters<LinkParams>,
    ) -> Result<CallToolResult, McpError> {
        run("cancel_payout_link", async {
            let c = self.caller();
            let id = id_arg("link_id", &p.link_id)?;
            Ok(json_result(
                &c.write(
                    Method::POST,
                    "cancel_payout_link",
                    &["payout-links", &id, "cancel"],
                    None,
                )
                .await?,
            ))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_link_body() {
        let p: CreateLinkParams = serde_json::from_value(json!({
            "counterparty_name": "Ann Lee", "account_id": "a1", "amount": "50", "currency": "gbp",
            "reference": "Refund", "payout_methods": ["bank account", "revolut", "revolut"], "expiry_days": 3
        }))
        .unwrap();
        assert_eq!(
            link_body(p, "r").unwrap(),
            json!({"counterparty_name": "Ann Lee", "request_id": "r", "account_id": "a1", "amount": 50,
                "currency": "GBP", "reference": "Refund", "payout_methods": ["bank_account", "revolut"], "expiry_period": "P3D"})
        );
    }
}
