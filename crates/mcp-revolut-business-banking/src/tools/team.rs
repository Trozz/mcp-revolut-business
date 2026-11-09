//! The business's team members and their expenses.

use super::{BankingServer, Query, choice, count_arg, id_arg, json_result, push, run, when_arg};
use crate::view;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::Value;

pub const EXPENSE_STATES: &[&str] = &[
    "missing_info",
    "awaiting_review",
    "rejected",
    "pending_reimbursement",
    "approved",
    "reverted",
    "refund_requested",
    "refunded",
];

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListTeamParams {
    /// Only members invited before this date-time; for the next page pass the oldest
    /// created_at returned.
    pub created_before: Option<String>,
    /// How many: 1 to 1000. Default 100.
    pub count: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListExpensesParams {
    /// Only expenses on or after this date or date-time.
    pub from: Option<String>,
    /// Only expenses before this date or date-time; for older ones pass the oldest expense_date
    /// returned.
    pub to: Option<String>,
    /// Only this state: missing_info, awaiting_review, rejected, pending_reimbursement, approved,
    /// reverted, refund_requested or refunded.
    pub state: Option<String>,
    /// How many: 1 to 500. Default 100.
    pub count: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ExpenseParams {
    /// The expense id, from list_expenses.
    pub expense_id: String,
}

fn more_hint(items: &[Value], count: u32, field: &str, arg: &str) -> Option<String> {
    (items.len() as u32 >= count).then(|| {
        let oldest = items
            .last()
            .and_then(|t| t.get(field))
            .and_then(Value::as_str)
            .unwrap_or_default();
        format!("there may be more: call again with {arg} {oldest}")
    })
}

#[tool_router(router = team_router, vis = "pub(crate)")]
impl BankingServer {
    #[tool(
        description = "The business's team members: id, name, email, state and role.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn list_team_members(
        &self,
        Parameters(p): Parameters<ListTeamParams>,
    ) -> Result<CallToolResult, McpError> {
        run("list_team_members", async {
            let c = self.caller();
            let count = count_arg(p.count, 100, 1000)?;
            let mut q: Query = vec![("limit", count.to_string())];
            push(
                &mut q,
                "created_before",
                when_arg("created_before", p.created_before.as_deref())?,
            );
            let items = view::items(
                c.get("team_members", &["team-members"], &q).await?,
                "team_members",
            );
            let more = more_hint(&items, count, "created_at", "created_before");
            Ok(json_result(&view::list(items, more)))
        })
        .await
    }

    #[tool(
        description = "Expenses team members submitted against card payments and reimbursements, newest first: state, description, who paid, merchant, date, amount, and splits with category and tax rate. Revolut serves expenses only on live accounts, not in the sandbox.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn list_expenses(
        &self,
        Parameters(p): Parameters<ListExpensesParams>,
    ) -> Result<CallToolResult, McpError> {
        run("list_expenses", async {
            let c = self.caller();
            let count = count_arg(p.count, 100, 500)?;
            let mut q: Query = vec![("count", count.to_string())];
            push(&mut q, "from", when_arg("from", p.from.as_deref())?);
            push(&mut q, "to", when_arg("to", p.to.as_deref())?);
            push(
                &mut q,
                "state",
                choice("state", p.state.as_deref(), EXPENSE_STATES)?.map(str::to_string),
            );
            let items = view::items(c.get("expenses", &["expenses"], &q).await?, "expenses");
            let more = more_hint(&items, count, "expense_date", "to");
            Ok(json_result(&view::list(items, more)))
        })
        .await
    }

    #[tool(
        description = "One expense by id, with its splits, categories, tax rates and receipt ids.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn get_expense(
        &self,
        Parameters(p): Parameters<ExpenseParams>,
    ) -> Result<CallToolResult, McpError> {
        run("get_expense", async {
            let c = self.caller();
            let id = id_arg("expense_id", &p.expense_id)?;
            Ok(json_result(
                &c.one("expense", &["expenses", &id], &[]).await?,
            ))
        })
        .await
    }
}
