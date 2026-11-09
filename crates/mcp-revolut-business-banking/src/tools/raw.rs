//! Read any other Revolut Business endpoint.

use super::{BankingServer, json_result, run};
use crate::error::RvError;
use crate::view;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ApiGetParams {
    /// The path below /api/1.0, e.g. counterparties/countries, transfer-reasons or
    /// team-members/<id>.
    pub path: String,
    /// Query parameters, by name.
    pub query: Option<BTreeMap<String, String>>,
}

#[tool_router(router = raw_router, vis = "pub(crate)")]
impl BankingServer {
    #[tool(
        description = "GETs any Revolut Business API endpoint the other tools do not cover, such as transfer-reasons, counterparties/countries, roles or team-members/<id>, and returns its JSON. Read only.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn api_get(
        &self,
        Parameters(p): Parameters<ApiGetParams>,
    ) -> Result<CallToolResult, McpError> {
        run("api_get", async {
            let c = self.caller();
            let segments = c.client().raw_segments(&p.path)?;
            let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
            let mut q: Vec<(&str, String)> = Vec::new();
            for (k, v) in p.query.iter().flatten() {
                if k.len() > 64 || v.len() > 500 {
                    return Err(RvError::invalid("query names and values are too long"));
                }
                if !k.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
                    return Err(RvError::invalid(format!("query name {k:?} is not valid")));
                }
                q.push((k.as_str(), v.clone()));
            }
            let v = c.get("api_get", &refs, &q).await?;
            Ok(json_result(&match v {
                serde_json::Value::Array(items) => {
                    view::list(items.into_iter().map(view::resource).collect(), None)
                }
                other => view::resource(other),
            }))
        })
        .await
    }
}
