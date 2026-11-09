//! Statements: a summary of the money in and out over a period, and a statement of one account as
//! CSV, ready for accounting software. Both page through every transaction in the period on the
//! server, so the model gets totals or a file instead of thousands of rows.

use super::{
    BankingServer, Caller, MAX_RESULT_CHARS, Query, choice, id_arg, json_result, opt_id, push, run,
    when_arg,
};
use crate::error::{self, RvError};
use crate::view::{minor, money};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Revolut serves at most this many transactions a page.
const PAGE: u32 = 1000;
/// Most transactions read for one summary or statement.
pub const MAX_TRANSACTIONS: usize = 20_000;
/// Rows listed in each "largest" or "failed" section of a summary.
const TOP: usize = 10;
/// Merchants listed in a summary.
const TOP_MERCHANTS: usize = 15;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SummaryParams {
    /// The start of the period, a date or ISO 8601 date-time (inclusive).
    pub from: String,
    /// The end of the period, a date or date-time (exclusive). Default: now.
    pub to: Option<String>,
    /// Only this account (id from list_accounts). Default: every account.
    pub account_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct StatementParams {
    /// The account (id from list_accounts). A statement is of one account, in its currency.
    pub account_id: String,
    /// The first day of the statement, YYYY-MM-DD (inclusive).
    pub from: String,
    /// The last day of the statement, YYYY-MM-DD (inclusive). Default: today.
    pub to: Option<String>,
    /// freeagent: date (dd/mm/yyyy), amount and description, without a header, as FreeAgent's
    /// bank statement upload takes it. full: a header and every detail (type, reference, fee,
    /// balance, ids). Default: full.
    pub format: Option<String>,
}

/// Every transaction from `from` until `to` (exclusive), newest first, and whether the limit cut
/// it short. Pages by moving `to` back to the oldest transaction seen, plus a millisecond so ones
/// sharing that instant are not lost, and drops the repeats by id.
pub(crate) async fn all_transactions(
    c: &Caller,
    from: &str,
    to: Option<&str>,
    account: Option<&str>,
    limit: usize,
) -> error::Result<(Vec<Value>, bool)> {
    let mut out: Vec<Value> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut upper = to.map(str::to_string);
    loop {
        let mut q: Query = vec![("count", PAGE.to_string()), ("from", from.to_string())];
        push(&mut q, "to", upper.clone());
        push(&mut q, "account", account.map(str::to_string));
        let page = match c.get("transactions", &["transactions"], &q).await? {
            Value::Array(items) => items,
            _ => Vec::new(),
        };
        let got = page.len();
        let mut fresh = 0;
        let mut oldest: Option<DateTime<Utc>> = None;
        for t in page {
            if let Some(at) = t
                .get("created_at")
                .and_then(Value::as_str)
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            {
                let at = at.with_timezone(&Utc);
                oldest = Some(oldest.map_or(at, |o| o.min(at)));
            }
            let id = t
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if id.is_empty() || seen.insert(id) {
                fresh += 1;
                out.push(t);
            }
        }
        if out.len() >= limit {
            out.truncate(limit);
            return Ok((out, true));
        }
        if (got as u32) < PAGE {
            return Ok((out, false));
        }
        let Some(oldest) = oldest else {
            return Ok((out, false));
        };
        if fresh == 0 {
            // A whole page at one instant: cannot page further without losing some.
            return Ok((out, true));
        }
        upper = Some(
            (oldest + Duration::milliseconds(1))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        );
    }
}

/// Running totals for one currency, in minor units.
#[derive(Default)]
struct Totals {
    money_in: i64,
    money_out: i64,
    fees: i64,
    own_in: i64,
    own_out: i64,
    count: u64,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

/// The summary of `txs`, counting only legs on `account` when given.
fn summarise(txs: &[Value], account: Option<&str>) -> Value {
    let mut totals: BTreeMap<String, Totals> = BTreeMap::new();
    let mut by_type: BTreeMap<String, (u64, BTreeMap<String, i64>)> = BTreeMap::new();
    let mut merchants: HashMap<(String, String), (i64, u64)> = HashMap::new();
    let mut largest_out: Vec<(i64, Value)> = Vec::new();
    let mut largest_in: Vec<(i64, Value)> = Vec::new();
    let mut not_completed: Vec<Value> = Vec::new();
    let mut states: BTreeMap<String, u64> = BTreeMap::new();
    let mut first: Option<&str> = None;
    let mut last: Option<&str> = None;
    for t in txs {
        let state = s(t, "state");
        *states.entry(state.to_string()).or_default() += 1;
        let at = s(t, "created_at");
        if !at.is_empty() {
            first = Some(first.map_or(at, |f| f.min(at)));
            last = Some(last.map_or(at, |l| l.max(at)));
        }
        if state != "completed" {
            if matches!(state, "declined" | "failed" | "reverted") && not_completed.len() < TOP * 2
            {
                not_completed.push(json!({
                    "id": t.get("id"), "created_at": t.get("created_at"), "type": t.get("type"),
                    "state": state, "reason_code": t.get("reason_code"),
                    "description": t.pointer("/legs/0/description"),
                    "amount": t.pointer("/legs/0/amount"), "currency": t.pointer("/legs/0/currency"),
                }));
            }
            continue;
        }
        let kind = s(t, "type").to_string();
        let merchant = t
            .pointer("/merchant/name")
            .and_then(Value::as_str)
            .map(str::to_string);
        for leg in t
            .get("legs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if account.is_some_and(|a| s(leg, "account_id") != a) {
                continue;
            }
            let currency = s(leg, "currency").to_string();
            let Some(amount) = minor(leg.get("amount")) else {
                continue;
            };
            let fee = minor(leg.get("fee")).unwrap_or(0).abs();
            let own = leg
                .pointer("/counterparty/account_type")
                .and_then(Value::as_str)
                == Some("self")
                || kind == "exchange";
            let tot = totals.entry(currency.clone()).or_default();
            tot.count += 1;
            tot.fees += fee;
            match (own, amount >= 0) {
                (true, true) => tot.own_in += amount,
                (true, false) => tot.own_out += -amount,
                (false, true) => tot.money_in += amount,
                (false, false) => tot.money_out += -amount,
            }
            let entry = by_type.entry(kind.clone()).or_default();
            entry.0 += 1;
            *entry.1.entry(currency.clone()).or_default() += amount;
            if !own {
                let row = json!({
                    "id": t.get("id"), "date": at.get(..10), "type": kind,
                    "description": leg.get("description"),
                    "merchant": merchant, "amount": money(amount), "currency": currency,
                });
                if amount < 0 {
                    largest_out.push((-amount, row));
                    if let Some(m) = &merchant {
                        let e = merchants.entry((m.clone(), currency.clone())).or_default();
                        e.0 += -amount;
                        e.1 += 1;
                    }
                } else {
                    largest_in.push((amount, row));
                }
            }
        }
    }
    let top = |mut v: Vec<(i64, Value)>| -> Vec<Value> {
        v.sort_by_key(|a| std::cmp::Reverse(a.0));
        v.into_iter()
            .take(TOP)
            .map(|(_, r)| crate::view::resource(r))
            .collect()
    };
    let mut merchant_rows: Vec<((String, String), (i64, u64))> = merchants.into_iter().collect();
    merchant_rows.sort_by_key(|a| std::cmp::Reverse(a.1.0));
    let per_currency: Map<String, Value> = totals
        .into_iter()
        .map(|(c, t)| {
            (
                c,
                json!({
                    "money_in": money(t.money_in),
                    "money_out": money(t.money_out),
                    "net": money(t.money_in - t.money_out),
                    "fees": money(t.fees),
                    "between_own_accounts_in": money(t.own_in),
                    "between_own_accounts_out": money(t.own_out),
                    "legs": t.count,
                }),
            )
        })
        .collect();
    let by_type: Map<String, Value> = by_type
        .into_iter()
        .map(|(k, (n, sums))| {
            let net: Map<String, Value> = sums
                .into_iter()
                .map(|(c, m)| (c, Value::from(money(m))))
                .collect();
            (k, json!({"count": n, "net": net}))
        })
        .collect();
    json!({
        "transactions": txs.len(),
        "first": first,
        "last": last,
        "states": states,
        "by_currency": per_currency,
        "by_type": by_type,
        "top_merchants": merchant_rows.into_iter().take(TOP_MERCHANTS).map(|((m, c), (spent, n))| json!({
            "merchant": m, "currency": c, "spent": money(spent), "payments": n
        })).collect::<Vec<_>>(),
        "largest_out": top(largest_out),
        "largest_in": top(largest_in),
        "declined_failed_reverted": not_completed,
        "notes": "Totals count completed transactions only. Transfers between the business's own accounts and exchanges are counted apart from money in and out. Fees are also included in the amounts.",
    })
}

/// A CSV field: quoted when needed, and a leading =, +, - or @ in text defused so spreadsheets do
/// not run it as a formula.
fn csv_text(v: &str) -> String {
    let mut t: String = v.chars().filter(|c| !c.is_control()).collect();
    if t.starts_with(['=', '+', '-', '@']) {
        t.insert(0, '\'');
    }
    if t.contains([',', '"']) {
        format!("\"{}\"", t.replace('"', "\"\""))
    } else {
        t
    }
}

/// The statement lines of `txs` on `account`, oldest first: date, amount and the rest. A fee
/// becomes its own line, as banks show it.
struct Line {
    date: NaiveDate,
    kind: String,
    state: String,
    description: String,
    reference: String,
    amount: i64,
    fee: i64,
    currency: String,
    balance: Option<String>,
    id: String,
}

fn lines(txs: &[Value], account: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for t in txs.iter().rev() {
        if s(t, "state") != "completed" {
            continue;
        }
        let when = t
            .get("completed_at")
            .or_else(|| t.get("created_at"))
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc).date_naive());
        let Some(date) = when else { continue };
        for leg in t
            .get("legs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if s(leg, "account_id") != account {
                continue;
            }
            let Some(amount) = minor(leg.get("amount")) else {
                continue;
            };
            let merchant = t.pointer("/merchant/name").and_then(Value::as_str);
            let description = [
                s(leg, "description"),
                merchant.unwrap_or_default(),
                s(t, "reference"),
                s(t, "type"),
            ]
            .into_iter()
            .find(|d| !d.trim().is_empty())
            .unwrap_or("Revolut transaction")
            .trim()
            .to_string();
            out.push(Line {
                date,
                kind: s(t, "type").to_string(),
                state: s(t, "state").to_string(),
                description,
                reference: s(t, "reference").to_string(),
                amount,
                fee: minor(leg.get("fee")).unwrap_or(0).abs(),
                currency: s(leg, "currency").to_string(),
                balance: leg.get("balance").and_then(|b| minor(Some(b))).map(money),
                id: s(t, "id").to_string(),
            });
        }
    }
    out
}

fn freeagent_csv(lines: &[Line]) -> String {
    let mut out = String::new();
    for l in lines {
        let d = l.date.format("%d/%m/%Y");
        out.push_str(&format!(
            "{d},{},{}\n",
            money(l.amount),
            csv_text(&l.description)
        ));
        if l.fee != 0 {
            out.push_str(&format!(
                "{d},{},{}\n",
                money(-l.fee),
                csv_text(&format!("Fee: {}", l.description))
            ));
        }
    }
    out
}

fn full_csv(lines: &[Line]) -> String {
    let mut out = String::from(
        "date,type,state,description,reference,amount,fee,currency,balance_after,transaction_id\n",
    );
    for l in lines {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{}\n",
            l.date.format("%Y-%m-%d"),
            csv_text(&l.kind),
            csv_text(&l.state),
            csv_text(&l.description),
            csv_text(&l.reference),
            money(l.amount),
            money(l.fee),
            csv_text(&l.currency),
            l.balance.as_deref().unwrap_or(""),
            csv_text(&l.id),
        ));
    }
    out
}

/// A statement day as the start of that day, or the start of the next for the last day.
fn day(name: &str, v: &str) -> error::Result<NaiveDate> {
    NaiveDate::parse_from_str(v.trim(), "%Y-%m-%d")
        .map_err(|_| RvError::invalid(format!("{name} must be a date as YYYY-MM-DD, got {v:?}")))
}

#[tool_router(router = statements_router, vis = "pub(crate)")]
impl BankingServer {
    #[tool(
        description = "Totals for a period, worked out on the server from every transaction in it (up to 20,000): per currency money in, money out, net and fees, with transfers between the business's own accounts and exchanges kept apart; counts and net by type; top merchants by card spend; the largest payments out and in; and every declined, failed or reverted transaction. Use it for monthly reviews and spending questions instead of listing transactions.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn summarise_transactions(
        &self,
        Parameters(p): Parameters<SummaryParams>,
    ) -> Result<CallToolResult, McpError> {
        run("summarise_transactions", async {
            let c = self.caller();
            let from = when_arg("from", Some(&p.from))?
                .ok_or_else(|| RvError::invalid("from is required"))?;
            let to = when_arg("to", p.to.as_deref())?;
            if let Some(t) = &to
                && from.get(..10) > t.get(..10)
            {
                return Err(RvError::invalid("from must not be after to"));
            }
            let account = opt_id("account_id", p.account_id.as_deref())?;
            let (txs, truncated) = all_transactions(
                &c,
                &from,
                to.as_deref(),
                account.as_deref(),
                MAX_TRANSACTIONS,
            )
            .await?;
            let mut out = summarise(&txs, account.as_deref());
            out["period"] = json!({"from": from, "to": to, "account_id": account});
            if truncated {
                out["truncated"] = Value::from(format!(
                    "only the newest {MAX_TRANSACTIONS} transactions were read: split the period"
                ));
            }
            Ok(json_result(&out))
        })
        .await
    }

    #[tool(
        description = "A statement of one account as CSV, oldest first, for completed transactions between two dates. format freeagent gives date (dd/mm/yyyy), amount and description without a header, ready for FreeAgent's bank statement upload (fees as their own lines); full adds type, reference, fee, balance after and transaction id with a header. Answers with a short summary and the CSV text.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn export_statement(
        &self,
        Parameters(p): Parameters<StatementParams>,
    ) -> Result<CallToolResult, McpError> {
        run("export_statement", async {
            let c = self.caller();
            let account = id_arg("account_id", &p.account_id)?;
            let first = day("from", &p.from)?;
            let last = match p.to.as_deref().filter(|t| !t.trim().is_empty()) {
                Some(t) => day("to", t)?,
                None => Utc::now().date_naive(),
            };
            if first > last {
                return Err(RvError::invalid("from must not be after to"));
            }
            let format = choice("format", p.format.as_deref(), &["freeagent", "full"])?
                .unwrap_or("full");
            let to = (last + Duration::days(1)).format("%Y-%m-%d").to_string();
            let (txs, truncated) = all_transactions(
                &c,
                &first.format("%Y-%m-%d").to_string(),
                Some(&to),
                Some(&account),
                MAX_TRANSACTIONS,
            )
            .await?;
            if truncated {
                return Err(RvError::invalid(format!(
                    "the period has more than {MAX_TRANSACTIONS} transactions: export a shorter one"
                )));
            }
            let lines = lines(&txs, &account);
            let csv = match format {
                "freeagent" => freeagent_csv(&lines),
                _ => full_csv(&lines),
            };
            if csv.len() > MAX_RESULT_CHARS {
                return Err(RvError::invalid(format!(
                    "the statement came to {} characters, more than a tool result can carry ({MAX_RESULT_CHARS}): export a shorter period, such as half of it",
                    csv.len()
                )));
            }
            let currencies: HashSet<&str> = lines.iter().map(|l| l.currency.as_str()).collect();
            let total: i64 = lines.iter().map(|l| l.amount - l.fee).sum();
            let summary = json!({
                "account_id": account,
                "from": first.format("%Y-%m-%d").to_string(),
                "to": last.format("%Y-%m-%d").to_string(),
                "format": format,
                "lines": lines.len(),
                "currency": currencies.into_iter().collect::<Vec<_>>(),
                "net_change": money(total),
                "closing_balance": lines.last().and_then(|l| l.balance.clone()),
            });
            Ok(CallToolResult::success(vec![
                ContentBlock::text(serde_json::to_string(&summary).unwrap_or_default()),
                ContentBlock::text(csv),
            ]))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(id: &str, kind: &str, state: &str, at: &str, legs: Value) -> Value {
        json!({"id": id, "type": kind, "state": state, "created_at": at, "completed_at": at, "legs": legs})
    }

    fn sample() -> Vec<Value> {
        vec![
            tx(
                "5",
                "card_payment",
                "declined",
                "2026-10-05T10:00:00Z",
                json!([{"account_id": "A", "amount": -9.99, "currency": "GBP", "description": "Odd shop"}]),
            ),
            {
                let mut t = tx(
                    "4",
                    "card_payment",
                    "completed",
                    "2026-10-04T10:00:00Z",
                    json!([{"account_id": "A", "amount": -3.5, "fee": 0, "currency": "GBP", "description": "Coffee", "balance": 196.5}]),
                );
                t["merchant"] = json!({"name": "Coffee Ltd"});
                t
            },
            tx(
                "3",
                "transfer",
                "completed",
                "2026-10-03T10:00:00Z",
                json!([{"account_id": "A", "amount": -50, "fee": 0.2, "currency": "GBP", "description": "To Acme, Ltd", "balance": 200,
                        "counterparty": {"account_type": "external"}}]),
            ),
            tx(
                "2",
                "exchange",
                "completed",
                "2026-10-02T10:00:00Z",
                json!([{"account_id": "A", "amount": -10, "currency": "GBP", "description": "To EUR", "balance": 250.2},
                       {"account_id": "B", "amount": 11.4, "currency": "EUR", "description": "From GBP", "balance": 11.4}]),
            ),
            tx(
                "1",
                "topup",
                "completed",
                "2026-10-01T10:00:00Z",
                json!([{"account_id": "A", "amount": 260.2, "currency": "GBP", "description": "=SUM(A1)", "balance": 260.2}]),
            ),
        ]
    }

    #[test]
    fn summaries_separate_own_moves_and_count_only_completed() {
        let v = summarise(&sample(), None);
        let gbp = &v["by_currency"]["GBP"];
        assert_eq!(gbp["money_in"], "260.20");
        assert_eq!(gbp["money_out"], "53.50");
        assert_eq!(gbp["net"], "206.70");
        assert_eq!(gbp["fees"], "0.20");
        assert_eq!(gbp["between_own_accounts_out"], "10.00");
        assert_eq!(v["by_currency"]["EUR"]["between_own_accounts_in"], "11.40");
        assert_eq!(v["states"]["declined"], 1);
        assert_eq!(v["declined_failed_reverted"][0]["id"], "5");
        assert_eq!(
            v["top_merchants"][0],
            json!({"merchant": "Coffee Ltd", "currency": "GBP", "spent": "3.50", "payments": 1})
        );
        assert_eq!(v["largest_out"][0]["amount"], "-50.00");
        let only_b = summarise(&sample(), Some("B"));
        assert!(only_b["by_currency"].get("GBP").is_none());
    }

    #[test]
    fn statements_for_freeagent_and_in_full() {
        let l = lines(&sample(), "A");
        assert_eq!(l.len(), 4, "declined left out, only account A");
        let fa = freeagent_csv(&l);
        assert_eq!(
            fa,
            "01/10/2026,260.20,'=SUM(A1)\n02/10/2026,-10.00,To EUR\n03/10/2026,-50.00,\"To Acme, Ltd\"\n03/10/2026,-0.20,\"Fee: To Acme, Ltd\"\n04/10/2026,-3.50,Coffee\n"
        );
        let full = full_csv(&l);
        assert!(full.starts_with("date,type,state,description"));
        assert!(
            full.contains("2026-10-04,card_payment,completed,Coffee,,-3.50,0.00,GBP,196.50,4\n"),
            "{full}"
        );
    }

    #[test]
    fn csv_text_is_quoted_and_defused() {
        assert_eq!(csv_text("plain"), "plain");
        assert_eq!(csv_text("a,b"), "\"a,b\"");
        assert_eq!(csv_text("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_text("+44 call"), "'+44 call");
        assert_eq!(csv_text("line\nbreak"), "linebreak");
    }
}
