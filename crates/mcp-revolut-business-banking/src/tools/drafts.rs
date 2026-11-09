//! Payment drafts: payments prepared here and approved by someone in the Revolut Business app.

use super::{
    BankingServer, amount_arg, choice, currency_arg, fields, id_arg, json_result, opt_id, opt_s,
    run, s, text_limited, when_arg,
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

/// Most payments in one draft.
pub const MAX_DRAFT_PAYMENTS: usize = 100;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListDraftsParams {
    /// Where the drafts came from: api, integration, email or all. Default: Revolut's (api).
    pub source: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DraftParams {
    /// The draft id, from list_payment_drafts or create_payment_draft.
    pub draft_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DraftPayment {
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
    pub reference: String,
    /// Who pays the transfer fees for international payments: shared or debtor.
    pub charge_bearer: Option<String>,
    /// A transfer reason code, where the destination needs one (list_transfer_reasons).
    pub transfer_reason_code: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateDraftParams {
    /// A title for the draft, shown in the Revolut Business app.
    pub title: Option<String>,
    /// The date to send on once approved, YYYY-MM-DD. Default: as soon as approved.
    pub schedule_for: Option<String>,
    /// The payments, 1 to 100.
    pub payments: Vec<DraftPayment>,
}

pub(crate) fn receiver(counterparty_id: &str, account_id: Option<&str>) -> Result<Value, RvError> {
    Ok(Value::Object(fields(vec![
        (
            "counterparty_id",
            s(id_arg("counterparty_id", counterparty_id)?),
        ),
        (
            "account_id",
            opt_s(opt_id("counterparty_account_id", account_id)?),
        ),
    ])))
}

fn draft_body(p: CreateDraftParams) -> Result<Value, RvError> {
    if p.payments.is_empty() || p.payments.len() > MAX_DRAFT_PAYMENTS {
        return Err(RvError::invalid(format!(
            "a draft holds 1 to {MAX_DRAFT_PAYMENTS} payments"
        )));
    }
    let schedule = when_arg("schedule_for", p.schedule_for.as_deref())?;
    if schedule.as_ref().is_some_and(|d| d.len() != 10) {
        return Err(RvError::invalid(
            "schedule_for must be a date as YYYY-MM-DD",
        ));
    }
    let mut payments = Vec::new();
    for (i, pay) in p.payments.into_iter().enumerate() {
        let n = i + 1;
        let reference = text_limited("reference", Some(&pay.reference), 100)?
            .ok_or_else(|| RvError::invalid(format!("payment {n}: reference must not be empty")))?;
        payments.push(Value::Object(fields(vec![
            ("account_id", s(id_arg("account_id", &pay.account_id)?)),
            (
                "receiver",
                Some(receiver(
                    &pay.counterparty_id,
                    pay.counterparty_account_id.as_deref(),
                )?),
            ),
            ("amount", Some(amount_arg("amount", &pay.amount)?)),
            ("currency", s(currency_arg("currency", &pay.currency)?)),
            ("reference", s(reference)),
            (
                "charge_bearer",
                choice(
                    "charge_bearer",
                    pay.charge_bearer.as_deref(),
                    &["shared", "debtor"],
                )?
                .and_then(s),
            ),
            (
                "transfer_reason_code",
                super::quotes::reason_code(pay.transfer_reason_code.as_deref())?,
            ),
        ])));
    }
    Ok(Value::Object(fields(vec![
        (
            "title",
            opt_s(text_limited("title", p.title.as_deref(), 100)?),
        ),
        ("schedule_for", opt_s(schedule)),
        ("payments", Some(Value::Array(payments))),
    ])))
}

#[tool_router(router = drafts_router, vis = "pub(crate)")]
impl BankingServer {
    #[tool(
        description = "Payment drafts waiting to be approved and sent: id, title, scheduled date, number of payments and source.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn list_payment_drafts(
        &self,
        Parameters(p): Parameters<ListDraftsParams>,
    ) -> Result<CallToolResult, McpError> {
        run("list_payment_drafts", async {
            let c = self.caller();
            let mut q = Vec::new();
            if let Some(src) = choice(
                "source",
                p.source.as_deref(),
                &["api", "integration", "email", "all"],
            )? {
                q.push(("source", src.to_string()));
            }
            let items = view::items(
                c.get("payment_drafts", &["payment-drafts"], &q).await?,
                "payment_orders",
            );
            Ok(json_result(&view::list(items, None)))
        })
        .await
    }

    #[tool(
        description = "One payment draft: its title, scheduled date and each payment with amount, account, recipient, reference, state, and the exchange rate and fee if it converts currency.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn get_payment_draft(
        &self,
        Parameters(p): Parameters<DraftParams>,
    ) -> Result<CallToolResult, McpError> {
        run("get_payment_draft", async {
            let c = self.caller();
            let id = id_arg("draft_id", &p.draft_id)?;
            Ok(json_result(
                &c.one("payment_draft", &["payment-drafts", &id], &[])
                    .await?,
            ))
        })
        .await
    }

    #[tool(
        description = "Prepares one or more payments to counterparties as a draft. Nothing is sent: someone with approval rights reviews and approves the draft in the Revolut Business app, which then sends it. The safe way to pay. Show the person each payment (from, to, amount, currency, reference) and get their agreement first.",
        annotations(open_world_hint = true)
    )]
    pub async fn create_payment_draft(
        &self,
        Parameters(p): Parameters<CreateDraftParams>,
    ) -> Result<CallToolResult, McpError> {
        run("create_payment_draft", async {
            let c = self.caller();
            let body = draft_body(p)?;
            let mut out = c
                .write(
                    Method::POST,
                    "create_payment_draft",
                    &["payment-drafts"],
                    Some(&body),
                )
                .await?;
            out["next"] = Value::from(
                "approve the draft in the Revolut Business app (Payments, Drafts) to send it",
            );
            Ok(json_result(&out))
        })
        .await
    }

    #[tool(
        description = "Deletes a payment draft that has not been sent. Confirm with the person first.",
        annotations(
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    pub async fn delete_payment_draft(
        &self,
        Parameters(p): Parameters<DraftParams>,
    ) -> Result<CallToolResult, McpError> {
        run("delete_payment_draft", async {
            let c = self.caller();
            let id = id_arg("draft_id", &p.draft_id)?;
            Ok(json_result(
                &c.write(
                    Method::DELETE,
                    "delete_payment_draft",
                    &["payment-drafts", &id],
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

    const ACC: &str = "1714b6ee-ca1a-e2cf-0040-00d3af294457";
    const CP: &str = "b6523744-b159-e2f1-0040-0001920ddaa0";

    #[test]
    fn a_draft_body_is_shaped_as_revolut_wants() {
        let p: CreateDraftParams = serde_json::from_value(json!({
            "title": "Suppliers", "schedule_for": "2026-12-01",
            "payments": [{"account_id": ACC, "counterparty_id": CP, "amount": "12.50", "currency": "gbp", "reference": "INV-1"}]
        }))
        .unwrap();
        assert_eq!(
            draft_body(p).unwrap(),
            json!({"title": "Suppliers", "schedule_for": "2026-12-01", "payments": [
                {"account_id": ACC, "receiver": {"counterparty_id": CP}, "amount": 12.5, "currency": "GBP", "reference": "INV-1"}
            ]})
        );
    }

    #[test]
    fn bad_drafts_are_refused() {
        for bad in [
            json!({"payments": []}),
            json!({"payments": [{"account_id": ACC, "counterparty_id": CP, "amount": "0", "currency": "GBP", "reference": "x"}]}),
            json!({"payments": [{"account_id": ACC, "counterparty_id": CP, "amount": "1", "currency": "GBP", "reference": " "}]}),
            json!({"schedule_for": "2026-12-01T10:00:00Z", "payments": [{"account_id": ACC, "counterparty_id": CP, "amount": "1", "currency": "GBP", "reference": "x"}]}),
        ] {
            let p: CreateDraftParams = serde_json::from_value(bad.clone()).unwrap();
            assert!(draft_body(p).is_err(), "{bad}");
        }
    }
}
