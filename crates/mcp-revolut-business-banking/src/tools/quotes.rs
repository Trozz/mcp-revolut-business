//! Before paying: what a payment would cost, which details a country's payees need, and the
//! reasons some countries require for a transfer.

use super::drafts::receiver;
use super::{
    BankingServer, amount_arg, choice, currency_arg, fields, id_arg, json_result, run, s,
    text_limited,
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

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct QuoteParams {
    /// The account to pay from (id from list_accounts).
    pub account_id: String,
    /// The counterparty to pay (id from list_counterparties).
    pub counterparty_id: String,
    /// Which of the counterparty's accounts, when it has more than one.
    pub counterparty_account_id: Option<String>,
    /// The amount, e.g. 250.00.
    pub amount: String,
    /// The currency of the amount, e.g. GBP.
    pub currency: String,
    /// Who pays the fees for international payments: shared, debtor or prefer_debtor.
    pub charge_bearer: Option<String>,
    /// A transfer reason code, where the destination needs one (list_transfer_reasons).
    pub transfer_reason_code: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RequirementsParams {
    /// The bank's country, ISO 3166 alpha-2 such as US.
    pub country: String,
    /// The account's currency, e.g. USD.
    pub currency: String,
    /// business or personal.
    pub recipient_type: String,
    /// The payment route where a country has several, e.g. ach, fedwire or swift for the US.
    pub route: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReasonsParams {
    /// The destination country, ISO 3166 alpha-2 such as IN. The full list is long: give this
    /// or currency.
    pub country: Option<String>,
    /// The payment currency, e.g. INR.
    pub currency: Option<String>,
}

/// A reason code as Revolut spells them: lower case letters and `_`.
pub(crate) fn reason_code(v: Option<&str>) -> Result<Option<Value>, RvError> {
    let Some(t) = text_limited("transfer_reason_code", v, 40)? else {
        return Ok(None);
    };
    if !t.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
        return Err(RvError::invalid(
            "transfer_reason_code must be a code from list_transfer_reasons, such as services",
        ));
    }
    Ok(Some(Value::String(t)))
}

fn country(name: &str, v: &str) -> Result<String, RvError> {
    let t = v.trim().to_ascii_uppercase();
    if t.len() == 2 && t.chars().all(|c| c.is_ascii_alphabetic()) {
        Ok(t)
    } else {
        Err(RvError::invalid(format!(
            "{name} must be a two-letter ISO 3166 country code such as GB"
        )))
    }
}

#[tool_router(router = quotes_router, vis = "pub(crate)")]
impl BankingServer {
    #[tool(
        description = "What a payment to a counterparty would cost, before drafting or sending it: the fee and its breakdown, the estimated total, the exchange rate and amount after exchange if it converts currency, the estimated arrival date, and warnings such as a volatile rate. Nothing is paid.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn get_payment_quote(
        &self,
        Parameters(p): Parameters<QuoteParams>,
    ) -> Result<CallToolResult, McpError> {
        run("get_payment_quote", async {
            let c = self.caller();
            let body = Value::Object(fields(vec![
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
                    reason_code(p.transfer_reason_code.as_deref())?,
                ),
            ]));
            let a = c
                .send(
                    Method::POST,
                    "indicative_quote",
                    &["pay", "indicative-quote"],
                    &[],
                    Some(&body),
                )
                .await?;
            Ok(json_result(&view::resource(a)))
        })
        .await
    }

    #[tool(
        description = "Which details Revolut needs to save a counterparty in a given country and currency: each field's name (as create_counterparty takes it, directly or in more_fields), whether it is required, its format and its allowed values. Call it before saving a payee outside the UK.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn get_counterparty_requirements(
        &self,
        Parameters(p): Parameters<RequirementsParams>,
    ) -> Result<CallToolResult, McpError> {
        run("get_counterparty_requirements", async {
            let c = self.caller();
            let kind = choice(
                "recipient_type",
                Some(&p.recipient_type),
                &["business", "personal"],
            )?
            .unwrap_or("business");
            let mut q = vec![
                ("country", country("country", &p.country)?),
                ("currency", currency_arg("currency", &p.currency)?),
                ("recipient_type", kind.to_string()),
            ];
            if let Some(r) = text_limited("route", p.route.as_deref(), 20)? {
                q.push(("route", r.to_ascii_lowercase()));
            }
            Ok(json_result(
                &c.one("counterparty_fields", &["counterparties", "fields"], &q)
                    .await?,
            ))
        })
        .await
    }

    #[tool(
        description = "The transfer reason codes some countries and currencies require on a payment (e.g. INR to India), for a destination country or currency. Pass the code as transfer_reason_code.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn list_transfer_reasons(
        &self,
        Parameters(p): Parameters<ReasonsParams>,
    ) -> Result<CallToolResult, McpError> {
        run("list_transfer_reasons", async {
            let c = self.caller();
            let country = p
                .country
                .as_deref()
                .filter(|v| !v.trim().is_empty())
                .map(|v| country("country", v))
                .transpose()?;
            let currency = p
                .currency
                .as_deref()
                .filter(|v| !v.trim().is_empty())
                .map(|v| currency_arg("currency", v))
                .transpose()?;
            if country.is_none() && currency.is_none() {
                return Err(RvError::invalid(
                    "give a country or currency: the full list of reasons is too long to return",
                ));
            }
            let items: Vec<Value> = view::items(
                c.get("transfer_reasons", &["transfer-reasons"], &[])
                    .await?,
                "transfer_reasons",
            )
            .into_iter()
            .filter(|r| {
                country
                    .as_deref()
                    .is_none_or(|k| r.get("country").and_then(Value::as_str) == Some(k))
                    && currency
                        .as_deref()
                        .is_none_or(|k| r.get("currency").and_then(Value::as_str) == Some(k))
            })
            .collect();
            Ok(json_result(&view::list(items, None)))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_codes_and_countries_are_checked() {
        assert_eq!(
            reason_code(Some("services")).unwrap(),
            Some(Value::from("services"))
        );
        assert_eq!(reason_code(None).unwrap(), None);
        assert!(reason_code(Some("Services!")).is_err());
        assert_eq!(country("c", "us").unwrap(), "US");
        assert!(country("c", "USA").is_err());
    }
}
