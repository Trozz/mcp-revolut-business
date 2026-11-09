//! Team administration: roles, invitations, role changes, suspension and removal.

use super::{
    BankingServer, Query, count_arg, id_arg, json_result, push, run, text_limited, when_arg,
};
use crate::error::RvError;
use crate::view;
use reqwest::Method;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct MemberParams {
    /// The team member id, from list_team_members.
    pub team_member_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListRolesParams {
    /// How many: 1 to 1000. Default 100.
    pub count: Option<u32>,
    /// Only roles created before this date-time, for the next page.
    pub created_before: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct InviteParams {
    /// The email address to invite.
    pub email: String,
    /// The role to give them (id from list_roles).
    pub role_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RoleParams {
    /// The team member id, from list_team_members.
    pub team_member_id: String,
    /// The new role (id from list_roles).
    pub role_id: String,
}

/// A role id: Revolut's ids are UUIDs or short names such as Owner.
fn role_arg(v: &str) -> Result<String, RvError> {
    let t = v.trim();
    if !t.is_empty()
        && t.len() <= 40
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        Ok(t.to_string())
    } else {
        Err(RvError::invalid("role_id must be an id from list_roles"))
    }
}

fn email_arg(v: &str) -> Result<String, RvError> {
    let t = text_limited("email", Some(v), 254)?
        .ok_or_else(|| RvError::invalid("email must not be empty"))?;
    let ok = t.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty()
            && domain.contains('.')
            && !domain.starts_with('.')
            && !domain.ends_with('.')
    }) && !t
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, ',' | ';' | '<' | '>'));
    if ok {
        Ok(t)
    } else {
        Err(RvError::invalid(format!("{t:?} is not an email address")))
    }
}

#[tool_router(router = team_admin_router, vis = "pub(crate)")]
impl BankingServer {
    #[tool(
        description = "One team member by id: name, email, state, role, department and manager.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn get_team_member(
        &self,
        Parameters(p): Parameters<MemberParams>,
    ) -> Result<CallToolResult, McpError> {
        run("get_team_member", async {
            let c = self.caller();
            let id = id_arg("team_member_id", &p.team_member_id)?;
            Ok(json_result(
                &c.one("team_member", &["team-members", &id], &[]).await?,
            ))
        })
        .await
    }

    #[tool(
        description = "The business's team roles, with their ids and names, for inviting someone or changing their role.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub async fn list_roles(
        &self,
        Parameters(p): Parameters<ListRolesParams>,
    ) -> Result<CallToolResult, McpError> {
        run("list_roles", async {
            let c = self.caller();
            let count = count_arg(p.count, 100, 1000)?;
            let mut q: Query = vec![("limit", count.to_string())];
            push(
                &mut q,
                "created_before",
                when_arg("created_before", p.created_before.as_deref())?,
            );
            let items = view::items(c.get("roles", &["roles"], &q).await?, "roles");
            Ok(json_result(&view::list(items, None)))
        })
        .await
    }

    #[tool(
        description = "Invites someone to the business's Revolut account by email, with a role (from list_roles). They get an email to join. Show the person the email and role and get their agreement first.",
        annotations(open_world_hint = true)
    )]
    pub async fn invite_team_member(
        &self,
        Parameters(p): Parameters<InviteParams>,
    ) -> Result<CallToolResult, McpError> {
        run("invite_team_member", async {
            let c = self.caller();
            let body = json!({"email": email_arg(&p.email)?, "role_id": role_arg(&p.role_id)?});
            Ok(json_result(
                &c.write(
                    Method::POST,
                    "invite_team_member",
                    &["team-members"],
                    Some(&body),
                )
                .await?,
            ))
        })
        .await
    }

    #[tool(
        description = "Gives a team member a different role (from list_roles), which changes what they can see and do. Confirm with the person first.",
        annotations(idempotent_hint = true, open_world_hint = true)
    )]
    pub async fn change_team_member_role(
        &self,
        Parameters(p): Parameters<RoleParams>,
    ) -> Result<CallToolResult, McpError> {
        run("change_team_member_role", async {
            let c = self.caller();
            let id = id_arg("team_member_id", &p.team_member_id)?;
            let body = json!({"role_id": role_arg(&p.role_id)?});
            c.write(
                Method::PUT,
                "team_member_role",
                &["team-members", &id, "role"],
                Some(&body),
            )
            .await?;
            Ok(json_result(
                &json!({"ok": true, "team_member_id": id, "role_id": body["role_id"]}),
            ))
        })
        .await
    }

    #[tool(
        description = "Suspends a team member: they cannot sign in or use their cards until unsuspended. Confirm with the person first.",
        annotations(
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    pub async fn suspend_team_member(
        &self,
        Parameters(p): Parameters<MemberParams>,
    ) -> Result<CallToolResult, McpError> {
        run("suspend_team_member", async {
            let c = self.caller();
            let id = id_arg("team_member_id", &p.team_member_id)?;
            c.write(
                Method::POST,
                "suspend_team_member",
                &["team-members", &id, "suspend"],
                None,
            )
            .await?;
            Ok(json_result(
                &json!({"ok": true, "team_member_id": id, "state": "suspended"}),
            ))
        })
        .await
    }

    #[tool(
        description = "Lets a suspended team member back in. Confirm with the person first.",
        annotations(idempotent_hint = true, open_world_hint = true)
    )]
    pub async fn unsuspend_team_member(
        &self,
        Parameters(p): Parameters<MemberParams>,
    ) -> Result<CallToolResult, McpError> {
        run("unsuspend_team_member", async {
            let c = self.caller();
            let id = id_arg("team_member_id", &p.team_member_id)?;
            c.write(
                Method::POST,
                "unsuspend_team_member",
                &["team-members", &id, "unsuspend"],
                None,
            )
            .await?;
            Ok(json_result(&json!({"ok": true, "team_member_id": id})))
        })
        .await
    }

    #[tool(
        description = "Removes a team member from the business for good, ending their access and cards. Suspending is reversible; this is not. Confirm with the person first, naming them.",
        annotations(
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    pub async fn remove_team_member(
        &self,
        Parameters(p): Parameters<MemberParams>,
    ) -> Result<CallToolResult, McpError> {
        run("remove_team_member", async {
            let c = self.caller();
            let id = id_arg("team_member_id", &p.team_member_id)?;
            c.write(
                Method::DELETE,
                "remove_team_member",
                &["team-members", &id],
                None,
            )
            .await?;
            Ok(json_result(
                &json!({"ok": true, "team_member_id": id, "state": "removed"}),
            ))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_and_roles_are_checked() {
        assert_eq!(email_arg(" ann@example.com ").unwrap(), "ann@example.com");
        for bad in [
            "",
            "ann",
            "ann@",
            "@example.com",
            "a b@example.com",
            "a@b",
            "a@x.com,b@y.com",
        ] {
            assert!(email_arg(bad).is_err(), "{bad}");
        }
        assert_eq!(role_arg("Owner").unwrap(), "Owner");
        assert!(role_arg("../x").is_err());
    }
}
