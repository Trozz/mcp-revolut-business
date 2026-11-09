//! Counterparties: the people and businesses the business pays.

use super::{
    BankingServer, Query, choice, count_arg, currency_arg, fields, id_arg, json_result, opt_s,
    push, run, s, text_arg, text_limited, when_arg,
};
use crate::error::RvError;
use crate::view;
use reqwest::Method;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListCounterpartiesParams {
    /// Only counterparties with this name. Revolut matches the whole name, so give it in full.
    pub name: Option<String>,
    /// Only those with this UK account number.
    pub account_no: Option<String>,
    /// Only those with this UK sort code.
    pub sort_code: Option<String>,
    /// Only those with this IBAN.
    pub iban: Option<String>,
    /// Only those created before this date-time, for the next page: pass the oldest created_at
    /// returned.
    pub created_before: Option<String>,
    /// How many: 1 to 1000. Default 100.
    pub count: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CounterpartyParams {
    /// The counterparty id, from list_counterparties.
    pub counterparty_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateCounterpartyParams {
    /// business or personal.
    pub profile_type: Option<String>,
    /// The business's name, for a business counterparty.
    pub company_name: Option<String>,
    /// The person's first name, for a personal counterparty.
    pub first_name: Option<String>,
    /// The person's last name, for a personal counterparty.
    pub last_name: Option<String>,
    /// A Revolut user's Revtag (without @), to pay them inside Revolut instead of by bank
    /// details.
    pub revtag: Option<String>,
    /// The bank's country, ISO 3166 alpha-2 such as GB. Needed with bank details.
    pub bank_country: Option<String>,
    /// The account's currency, e.g. GBP. Needed with bank details.
    pub currency: Option<String>,
    /// UK account number (8 digits).
    pub account_no: Option<String>,
    /// UK sort code (6 digits).
    pub sort_code: Option<String>,
    /// IBAN, for accounts outside the UK or in another currency.
    pub iban: Option<String>,
    /// BIC (SWIFT code), with an IBAN where needed.
    pub bic: Option<String>,
    /// US routing number, for USD accounts in the US.
    pub routing_number: Option<String>,
    /// The counterparty's address, which some countries and currencies require: fields
    /// street_line1, street_line2, city, region, postcode, country (ISO alpha-2).
    pub address: Option<Map<String, Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ValidateNameParams {
    /// UK account number (8 digits).
    pub account_no: String,
    /// UK sort code (6 digits).
    pub sort_code: String,
    /// The business name to check, for a business account.
    pub company_name: Option<String>,
    /// The person's first name, for a personal account.
    pub first_name: Option<String>,
    /// The person's last name, for a personal account.
    pub last_name: Option<String>,
}

/// Digits only, of exactly `len` characters once spaces and dashes are removed.
fn digits(name: &str, v: &str, len: usize) -> Result<String, RvError> {
    let d: String = v.chars().filter(|c| !matches!(c, ' ' | '-')).collect();
    if d.len() == len && d.chars().all(|c| c.is_ascii_digit()) {
        Ok(d)
    } else {
        Err(RvError::invalid(format!("{name} must be {len} digits")))
    }
}

fn opt_digits(name: &str, v: Option<&str>, len: usize) -> Result<Option<String>, RvError> {
    v.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| digits(name, s, len))
        .transpose()
}

/// A code of letters and digits, upper-cased, spaces removed (IBAN, BIC, country).
fn code(
    name: &str,
    v: Option<&str>,
    lens: std::ops::RangeInclusive<usize>,
) -> Result<Option<String>, RvError> {
    let Some(t) = v.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let c: String = t
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    if lens.contains(&c.len()) && c.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(Some(c))
    } else {
        Err(RvError::invalid(format!("{name} is not a valid code")))
    }
}

/// `individual_name` for a person.
fn person(first: Option<String>, last: Option<String>) -> Option<Value> {
    (first.is_some() || last.is_some()).then(|| {
        Value::Object(fields(vec![
            ("first_name", opt_s(first)),
            ("last_name", opt_s(last)),
        ]))
    })
}

fn counterparty_body(p: CreateCounterpartyParams) -> Result<Value, RvError> {
    let profile = choice(
        "profile_type",
        p.profile_type.as_deref(),
        &["business", "personal"],
    )?;
    let company = text_limited("company_name", p.company_name.as_deref(), 200)?;
    let first = text_limited("first_name", p.first_name.as_deref(), 100)?;
    let last = text_limited("last_name", p.last_name.as_deref(), 100)?;
    let revtag = text_limited("revtag", p.revtag.as_deref(), 100)?
        .map(|r| r.trim_start_matches('@').to_string());
    let individual = person(first, last);
    if company.is_none() && individual.is_none() && revtag.is_none() {
        return Err(RvError::invalid(
            "give company_name, first_name and last_name, or a revtag",
        ));
    }
    if company.is_some() && individual.is_some() {
        return Err(RvError::invalid(
            "give either company_name or a person's name, not both",
        ));
    }
    let account_no = opt_digits("account_no", p.account_no.as_deref(), 8)?;
    let sort_code = opt_digits("sort_code", p.sort_code.as_deref(), 6)?;
    let iban = code("iban", p.iban.as_deref(), 15..=34)?;
    let bic = code("bic", p.bic.as_deref(), 8..=11)?;
    let routing = opt_digits("routing_number", p.routing_number.as_deref(), 9)?;
    let country = code("bank_country", p.bank_country.as_deref(), 2..=2)?;
    let currency = p
        .currency
        .as_deref()
        .filter(|c| !c.trim().is_empty())
        .map(|c| currency_arg("currency", c))
        .transpose()?;
    let has_bank = account_no.is_some() || iban.is_some() || routing.is_some();
    if revtag.is_some() && has_bank {
        return Err(RvError::invalid("give a revtag or bank details, not both"));
    }
    if revtag.is_none() {
        if !has_bank {
            return Err(RvError::invalid(
                "give bank details (account_no and sort_code, or iban) or a revtag",
            ));
        }
        if account_no.is_some() != sort_code.is_some() && iban.is_none() && routing.is_none() {
            return Err(RvError::invalid("account_no and sort_code go together"));
        }
        if country.is_none() || currency.is_none() {
            return Err(RvError::invalid(
                "bank_country and currency are needed with bank details",
            ));
        }
    }
    if let Some(a) = &p.address {
        for k in a.keys() {
            if ![
                "street_line1",
                "street_line2",
                "city",
                "region",
                "postcode",
                "country",
            ]
            .contains(&k.as_str())
            {
                return Err(RvError::invalid(format!("address has no field {k}")));
            }
        }
    }
    let profile = profile.or(match (&company, &individual) {
        (Some(_), _) => Some("business"),
        (_, Some(_)) => Some("personal"),
        _ => None,
    });
    Ok(Value::Object(fields(vec![
        ("profile_type", profile.and_then(s)),
        ("company_name", opt_s(company)),
        ("individual_name", individual),
        ("revtag", opt_s(revtag)),
        ("bank_country", opt_s(country)),
        ("currency", opt_s(currency)),
        ("account_no", opt_s(account_no)),
        ("sort_code", opt_s(sort_code)),
        ("iban", opt_s(iban)),
        ("bic", opt_s(bic)),
        ("routing_number", opt_s(routing)),
        ("address", p.address.map(Value::Object)),
    ])))
}

#[tool_router(router = counterparties_router, vis = "pub(crate)")]
impl BankingServer {
    #[tool(
        description = "The business's saved counterparties (payees), newest first: id, name, type, state, and their accounts (id, currency, account number and sort code or IBAN and BIC) or Revtag. Payments go to a counterparty's account id. Filter by name or bank details.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn list_counterparties(
        &self,
        Parameters(p): Parameters<ListCounterpartiesParams>,
    ) -> Result<CallToolResult, McpError> {
        run("list_counterparties", async {
            let c = self.caller();
            let count = count_arg(p.count, 100, 1000)?;
            let mut q: Query = vec![("limit", count.to_string())];
            push(
                &mut q,
                "name",
                text_limited("name", p.name.as_deref(), 200)?,
            );
            push(
                &mut q,
                "account_no",
                opt_digits("account_no", p.account_no.as_deref(), 8)?,
            );
            push(
                &mut q,
                "sort_code",
                opt_digits("sort_code", p.sort_code.as_deref(), 6)?,
            );
            push(&mut q, "iban", code("iban", p.iban.as_deref(), 15..=34)?);
            push(
                &mut q,
                "created_before",
                when_arg("created_before", p.created_before.as_deref())?,
            );
            let items = view::items(
                c.get("counterparties", &["counterparties"], &q).await?,
                "counterparties",
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
        description = "One counterparty by id, with its accounts.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn get_counterparty(
        &self,
        Parameters(p): Parameters<CounterpartyParams>,
    ) -> Result<CallToolResult, McpError> {
        run("get_counterparty", async {
            let c = self.caller();
            let id = id_arg("counterparty_id", &p.counterparty_id)?;
            Ok(json_result(
                &c.one("counterparty", &["counterparty", &id], &[]).await?,
            ))
        })
        .await
    }

    #[tool(
        description = "Saves a new counterparty to pay: a business (company_name) or person (first_name, last_name) with bank details (UK: account_no and sort_code; elsewhere: iban and bic; US: account_no and routing_number), bank_country and currency; or a Revolut user by revtag. For UK accounts, check the name first with validate_account_name. Confirm the details with the person first.",
        annotations(open_world_hint = true)
    )]
    pub async fn create_counterparty(
        &self,
        Parameters(p): Parameters<CreateCounterpartyParams>,
    ) -> Result<CallToolResult, McpError> {
        run("create_counterparty", async {
            let c = self.caller();
            let body = counterparty_body(p)?;
            Ok(json_result(
                &c.write(
                    Method::POST,
                    "create_counterparty",
                    &["counterparty"],
                    Some(&body),
                )
                .await?,
            ))
        })
        .await
    }

    #[tool(
        description = "Deletes a saved counterparty. Payments already made to it are kept. Confirm with the person first.",
        annotations(
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    pub async fn delete_counterparty(
        &self,
        Parameters(p): Parameters<CounterpartyParams>,
    ) -> Result<CallToolResult, McpError> {
        run("delete_counterparty", async {
            let c = self.caller();
            let id = id_arg("counterparty_id", &p.counterparty_id)?;
            Ok(json_result(
                &c.write(
                    Method::DELETE,
                    "delete_counterparty",
                    &["counterparty", &id],
                    None,
                )
                .await?,
            ))
        })
        .await
    }

    #[tool(
        description = "Confirmation of Payee for a UK account: asks the recipient's bank whether the name matches the account number and sort code, before saving or paying it. The answer says matched, close match (with the name the bank holds), not matched, or could not check. Nothing is saved.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn validate_account_name(
        &self,
        Parameters(p): Parameters<ValidateNameParams>,
    ) -> Result<CallToolResult, McpError> {
        run("validate_account_name", async {
            let c = self.caller();
            let company = text_limited("company_name", p.company_name.as_deref(), 200)?;
            let individual = person(
                text_arg("first_name", p.first_name.as_deref())?,
                text_arg("last_name", p.last_name.as_deref())?,
            );
            if company.is_some() == individual.is_some() {
                return Err(RvError::invalid(
                    "give either company_name or the person's first_name and last_name",
                ));
            }
            let body = Value::Object(fields(vec![
                ("account_no", s(digits("account_no", &p.account_no, 8)?)),
                ("sort_code", s(digits("sort_code", &p.sort_code, 6)?)),
                ("company_name", opt_s(company)),
                ("individual_name", individual),
            ]));
            let a = c
                .send(
                    Method::POST,
                    "account_name_validation",
                    &["account-name-validation"],
                    &[],
                    Some(&body),
                )
                .await?;
            Ok(json_result(&view::resource(a)))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(v: Value) -> CreateCounterpartyParams {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_uk_business_counterparty() {
        let b = counterparty_body(params(json!({
            "company_name": "Acme Ltd", "account_no": "1234 5678", "sort_code": "12-34-56",
            "bank_country": "gb", "currency": "gbp"
        })))
        .unwrap();
        assert_eq!(
            b,
            json!({"profile_type": "business", "company_name": "Acme Ltd", "account_no": "12345678",
                "sort_code": "123456", "bank_country": "GB", "currency": "GBP"})
        );
    }

    #[test]
    fn a_person_by_revtag() {
        let b = counterparty_body(params(
            json!({"first_name": "Ann", "last_name": "Lee", "revtag": "@annlee"}),
        ))
        .unwrap();
        assert_eq!(b["revtag"], "annlee");
        assert_eq!(
            b["individual_name"],
            json!({"first_name": "Ann", "last_name": "Lee"})
        );
        assert_eq!(b["profile_type"], "personal");
    }

    #[test]
    fn incomplete_or_mixed_details_are_refused() {
        for bad in [
            json!({"company_name": "A"}),
            json!({"account_no": "12345678", "sort_code": "123456", "bank_country": "GB", "currency": "GBP"}),
            json!({"company_name": "A", "account_no": "12345678", "sort_code": "123456"}),
            json!({"company_name": "A", "account_no": "1234", "sort_code": "123456", "bank_country": "GB", "currency": "GBP"}),
            json!({"company_name": "A", "first_name": "B", "revtag": "x"}),
            json!({"company_name": "A", "revtag": "x", "iban": "GB33BUKB20201555555555"}),
            json!({"company_name": "A", "revtag": "x", "address": {"planet": "Mars"}}),
        ] {
            assert!(counterparty_body(params(bad.clone())).is_err(), "{bad}");
        }
    }
}
