//! Card administration: issuing virtual cards, changing limits and controls, and locking or
//! terminating cards.

use super::{
    BankingServer, amount_arg, currency_arg, fields, id_arg, json_result, opt_id, opt_s,
    request_id, run, s, text_limited,
};
use crate::error::RvError;
use reqwest::Method;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The periods a card's spending can be limited over.
pub const LIMIT_PERIODS: &[&str] = &[
    "single", "day", "week", "month", "quarter", "year", "all_time",
];

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct Limit {
    /// The most that may be spent, e.g. 500.
    pub amount: String,
    /// Its currency, e.g. GBP.
    pub currency: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct Reference {
    /// The reference's name, e.g. Project. Unique on the card.
    pub name: String,
    /// Its value, e.g. Website rebuild.
    pub value: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateCardParams {
    /// The team member who will hold the card (id from list_team_members). Leave out for a
    /// company card, and give contact_ids instead.
    pub holder_id: Option<String>,
    /// For a company card: up to 5 team members (ids from list_team_members) who can see its
    /// details in the app.
    pub contact_ids: Option<Vec<String>>,
    /// A label for the card, up to 30 characters.
    pub label: Option<String>,
    /// Spending limits by period: single (one payment), day, week, month, quarter, year or
    /// all_time, each with an amount and currency.
    pub spending_limits: Option<BTreeMap<String, Limit>>,
    /// The accounts the card spends from (ids from list_accounts). Default: Revolut's choice.
    pub account_ids: Option<Vec<String>>,
    /// The only countries the card may be used in, ISO 3166 alpha-2 codes.
    pub countries: Option<Vec<String>>,
    /// Up to 5 name and value pairs recorded on every transaction, for a company card.
    pub references: Option<Vec<Reference>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct UpdateCardParams {
    /// The card id, from list_cards.
    pub card_id: String,
    /// A new label, up to 30 characters.
    pub label: Option<String>,
    /// New spending limits by period (single, day, week, month, quarter, year, all_time), each
    /// with an amount and currency. Periods left out keep their limit.
    pub spending_limits: Option<BTreeMap<String, Limit>>,
    /// The accounts the card spends from, replacing the current ones.
    pub account_ids: Option<Vec<String>>,
    /// The only countries the card may be used in, replacing the current list.
    pub countries: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CardParams {
    /// The card id, from list_cards.
    pub card_id: String,
}

fn limits(v: Option<BTreeMap<String, Limit>>) -> Result<Option<Value>, RvError> {
    let Some(map) = v else { return Ok(None) };
    let mut out = Map::new();
    for (period, l) in map {
        let p = period.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        if !LIMIT_PERIODS.contains(&p.as_str()) {
            return Err(RvError::invalid(format!(
                "spending_limits periods are {}, not {period}",
                LIMIT_PERIODS.join(", ")
            )));
        }
        out.insert(
            p,
            json!({"amount": amount_arg("amount", &l.amount)?, "currency": currency_arg("currency", &l.currency)?}),
        );
    }
    Ok((!out.is_empty()).then_some(Value::Object(out)))
}

fn ids(name: &str, v: Option<Vec<String>>) -> Result<Option<Value>, RvError> {
    let Some(list) = v else { return Ok(None) };
    let mut out = Vec::new();
    for i in &list {
        out.push(Value::String(id_arg(name, i)?));
    }
    Ok((!out.is_empty()).then_some(Value::Array(out)))
}

fn countries(v: Option<Vec<String>>) -> Result<Option<Value>, RvError> {
    let Some(list) = v else { return Ok(None) };
    let mut out = Vec::new();
    for c in &list {
        let c = c.trim().to_ascii_uppercase();
        if c.len() != 2 || !c.chars().all(|x| x.is_ascii_alphabetic()) {
            return Err(RvError::invalid(
                "countries must be two-letter ISO 3166 codes such as GB",
            ));
        }
        out.push(Value::String(c));
    }
    Ok((!out.is_empty()).then_some(Value::Array(out)))
}

fn label(v: Option<&str>) -> Result<Option<Value>, RvError> {
    Ok(opt_s(text_limited("label", v, 30)?))
}

fn card_body(p: CreateCardParams, rid: &str) -> Result<Value, RvError> {
    let references = match p.references {
        None => None,
        Some(r) if r.len() > 5 => {
            return Err(RvError::invalid("a card takes at most 5 references"));
        }
        Some(r) => {
            let mut out = Vec::new();
            for x in r {
                let name = text_limited("reference name", Some(&x.name), 30)?
                    .ok_or_else(|| RvError::invalid("a reference needs a name"))?;
                let value = text_limited("reference value", Some(&x.value), 30)?
                    .ok_or_else(|| RvError::invalid("a reference needs a value"))?;
                out.push(json!({"name": name, "value": value}));
            }
            (!out.is_empty()).then_some(Value::Array(out))
        }
    };
    let holder = opt_id("holder_id", p.holder_id.as_deref())?;
    let contacts = ids("contact_ids", p.contact_ids)?;
    match (&holder, &contacts) {
        (None, None) => {
            return Err(RvError::invalid(
                "give holder_id for a team member's card, or contact_ids for a company card",
            ));
        }
        (Some(_), Some(_)) => {
            return Err(RvError::invalid(
                "contact_ids are for company cards: give holder_id or contact_ids, not both",
            ));
        }
        (None, Some(Value::Array(c))) if c.len() > 5 => {
            return Err(RvError::invalid("a card takes at most 5 contacts"));
        }
        _ => {}
    }
    Ok(Value::Object(fields(vec![
        ("request_id", s(rid)),
        ("virtual", Some(Value::Bool(true))),
        ("holder_id", opt_s(holder)),
        ("contact_ids", contacts),
        ("label", label(p.label.as_deref())?),
        ("spending_limits", limits(p.spending_limits)?),
        ("accounts", ids("account_ids", p.account_ids)?),
        ("countries", countries(p.countries)?),
        ("references", references),
    ])))
}

fn update_body(p: UpdateCardParams) -> Result<Value, RvError> {
    let body = fields(vec![
        ("label", label(p.label.as_deref())?),
        ("spending_limits", limits(p.spending_limits)?),
        ("accounts", ids("account_ids", p.account_ids)?),
        ("countries", countries(p.countries)?),
    ]);
    if body.is_empty() {
        return Err(RvError::invalid(
            "give at least one of label, spending_limits, account_ids or countries",
        ));
    }
    Ok(Value::Object(body))
}

#[tool_router(router = card_admin_router, vis = "pub(crate)")]
impl BankingServer {
    #[tool(
        description = "Issues a new virtual card, for a team member (holder_id) or the company (contact_ids: who can see it), with an optional label, spending limits, accounts, allowed countries and references. Show the person the holder, limits and accounts and get their agreement first. The card number is never shown here; it is in the Revolut Business app.",
        annotations(open_world_hint = true)
    )]
    pub async fn create_virtual_card(
        &self,
        Parameters(p): Parameters<CreateCardParams>,
    ) -> Result<CallToolResult, McpError> {
        run("create_virtual_card", async {
            let c = self.caller();
            let rid = request_id(None)?;
            let body = card_body(p, &rid)?;
            Ok(json_result(
                &c.write(Method::POST, "create_card", &["cards"], Some(&body))
                    .await?,
            ))
        })
        .await
    }

    #[tool(
        description = "Changes a card's label, spending limits, accounts or allowed countries. Only what is given changes. Confirm with the person first.",
        annotations(idempotent_hint = true, open_world_hint = true)
    )]
    pub async fn update_card(
        &self,
        Parameters(p): Parameters<UpdateCardParams>,
    ) -> Result<CallToolResult, McpError> {
        run("update_card", async {
            let c = self.caller();
            let id = id_arg("card_id", &p.card_id)?;
            let body = update_body(p)?;
            Ok(json_result(
                &c.write(Method::PATCH, "update_card", &["cards", &id], Some(&body))
                    .await?,
            ))
        })
        .await
    }

    #[tool(
        description = "Locks a card as an admin: unlike a freeze, which the cardholder can undo, only an admin can unlock it (unlock_card). Confirm with the person first.",
        annotations(
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    pub async fn lock_card(
        &self,
        Parameters(p): Parameters<CardParams>,
    ) -> Result<CallToolResult, McpError> {
        run("lock_card", async {
            let c = self.caller();
            let id = id_arg("card_id", &p.card_id)?;
            c.write(Method::POST, "lock_card", &["cards", &id, "lock"], None)
                .await?;
            Ok(json_result(
                &json!({"ok": true, "card_id": id, "state": "locked"}),
            ))
        })
        .await
    }

    #[tool(
        description = "Unlocks a card an admin locked. Confirm with the person first.",
        annotations(idempotent_hint = true, open_world_hint = true)
    )]
    pub async fn unlock_card(
        &self,
        Parameters(p): Parameters<CardParams>,
    ) -> Result<CallToolResult, McpError> {
        run("unlock_card", async {
            let c = self.caller();
            let id = id_arg("card_id", &p.card_id)?;
            c.write(Method::POST, "unlock_card", &["cards", &id, "unlock"], None)
                .await?;
            Ok(json_result(&json!({"ok": true, "card_id": id})))
        })
        .await
    }

    #[tool(
        description = "Terminates a card for good: it can never be used again, and cannot be brought back. For a lost card, freeze_card is usually enough. Confirm with the person first, naming the card's label and last four digits.",
        annotations(
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    pub async fn terminate_card(
        &self,
        Parameters(p): Parameters<CardParams>,
    ) -> Result<CallToolResult, McpError> {
        run("terminate_card", async {
            let c = self.caller();
            let id = id_arg("card_id", &p.card_id)?;
            c.write(Method::DELETE, "terminate_card", &["cards", &id], None)
                .await?;
            Ok(json_result(
                &json!({"ok": true, "card_id": id, "state": "terminated"}),
            ))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(v: Value) -> CreateCardParams {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_virtual_card_body() {
        let b = card_body(
            create(json!({
                "label": "Ads", "contact_ids": ["m1"], "spending_limits": {"Month": {"amount": "500", "currency": "gbp"}, "single": {"amount": "50.5", "currency": "GBP"}},
                "countries": ["gb", "ie"], "references": [{"name": "Project", "value": "Web"}]
            })),
            "r",
        )
        .unwrap();
        assert_eq!(
            b,
            json!({"request_id": "r", "virtual": true, "contact_ids": ["m1"], "label": "Ads",
                "spending_limits": {"month": {"amount": 500, "currency": "GBP"}, "single": {"amount": 50.5, "currency": "GBP"}},
                "countries": ["GB", "IE"], "references": [{"name": "Project", "value": "Web"}]})
        );
    }

    #[test]
    fn bad_cards_and_empty_updates_are_refused() {
        for bad in [
            json!({}),
            json!({"holder_id": "h", "contact_ids": ["m1"]}),
            json!({"holder_id": "h", "label": "x".repeat(31)}),
            json!({"holder_id": "h", "spending_limits": {"fortnight": {"amount": "1", "currency": "GBP"}}}),
            json!({"holder_id": "h", "spending_limits": {"day": {"amount": "-1", "currency": "GBP"}}}),
            json!({"holder_id": "h", "countries": ["GBR"]}),
            json!({"holder_id": "h", "references": [{"name": "a", "value": "1"}, {"name": "b", "value": "1"}, {"name": "c", "value": "1"}, {"name": "d", "value": "1"}, {"name": "e", "value": "1"}, {"name": "f", "value": "1"}]}),
        ] {
            assert!(card_body(create(bad.clone()), "r").is_err(), "{bad}");
        }
        let empty: UpdateCardParams = serde_json::from_value(json!({"card_id": "c"})).unwrap();
        assert!(update_body(empty).is_err());
    }
}
