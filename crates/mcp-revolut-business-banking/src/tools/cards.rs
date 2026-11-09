//! Cards: the business's cards, and freezing or unfreezing one.

use super::{BankingServer, Query, count_arg, id_arg, json_result, push, run, when_arg};
use crate::view;
use reqwest::Method;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListCardsParams {
    /// Only cards created before this date-time; for the next page pass the oldest created_at
    /// returned.
    pub created_before: Option<String>,
    /// How many: 1 to 100. Default 100.
    pub count: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CardParams {
    /// The card id, from list_cards.
    pub card_id: String,
}

#[tool_router(router = cards_router, vis = "pub(crate)")]
impl BankingServer {
    #[tool(
        description = "The business's cards (terminated ones left out): id, holder, label, virtual or physical, last four digits, expiry, state (active, frozen, locked...), linked accounts, spending limits and controls.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn list_cards(
        &self,
        Parameters(p): Parameters<ListCardsParams>,
    ) -> Result<CallToolResult, McpError> {
        run("list_cards", async {
            let c = self.caller();
            let count = count_arg(p.count, 100, 100)?;
            let mut q: Query = vec![("limit", count.to_string())];
            push(
                &mut q,
                "created_before",
                when_arg("created_before", p.created_before.as_deref())?,
            );
            let items = view::items(c.get("cards", &["cards"], &q).await?, "cards");
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
        description = "One card by id: holder, state, limits, controls and linked accounts. Never the card number or CVV.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn get_card(
        &self,
        Parameters(p): Parameters<CardParams>,
    ) -> Result<CallToolResult, McpError> {
        run("get_card", async {
            let c = self.caller();
            let id = id_arg("card_id", &p.card_id)?;
            Ok(json_result(&c.one("card", &["cards", &id], &[]).await?))
        })
        .await
    }

    #[tool(
        description = "Freezes an active card so it cannot be used until unfrozen, for example when it is lost. Confirm with the person first.",
        annotations(
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    pub async fn freeze_card(
        &self,
        Parameters(p): Parameters<CardParams>,
    ) -> Result<CallToolResult, McpError> {
        run("freeze_card", async {
            let c = self.caller();
            let id = id_arg("card_id", &p.card_id)?;
            c.write(Method::POST, "freeze_card", &["cards", &id, "freeze"], None)
                .await?;
            Ok(json_result(
                &json!({"ok": true, "card_id": id, "state": "frozen"}),
            ))
        })
        .await
    }

    #[tool(
        description = "Unfreezes a frozen card so it can be used again. Confirm with the person first.",
        annotations(idempotent_hint = true, open_world_hint = true)
    )]
    pub async fn unfreeze_card(
        &self,
        Parameters(p): Parameters<CardParams>,
    ) -> Result<CallToolResult, McpError> {
        run("unfreeze_card", async {
            let c = self.caller();
            let id = id_arg("card_id", &p.card_id)?;
            c.write(
                Method::POST,
                "unfreeze_card",
                &["cards", &id, "unfreeze"],
                None,
            )
            .await?;
            Ok(json_result(
                &json!({"ok": true, "card_id": id, "state": "active"}),
            ))
        })
        .await
    }
}
