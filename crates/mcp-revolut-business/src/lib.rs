//! The mcp-revolut-business server: Revolut Business tools and prompts on one
//! [`mcp_server_core::app::Server`]. The server holds the business's API key and signs every
//! request with it; the owner signs in to the server with `OAUTH_ADMIN_PASSWORD`.

use mcp_server_core::app::{Server, SignIn};
pub use mcp_server_core::combined::COMBINED;
use mcp_server_core::service::ServiceFactory;

pub const VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

pub const SERVICE_NAME: &str = "mcp-revolut-business";
/// Tracing target prefix: also matches `mcp_revolut_business_banking`.
pub const CRATE_TARGET: &str = "mcp_revolut_business";

/// Every service this binary can run, in mount order.
pub const SERVICES: &[(&str, ServiceFactory)] =
    &[("revolut", mcp_revolut_business_banking::from_env)];

/// The server: `REVOLUT_SERVICES`, `REVOLUT_COMBINED` and `REVOLUT_READ_ONLY` choose what runs.
/// The API key belongs to one business, so one person, the owner, signs in.
pub fn server() -> Server {
    Server::new(SERVICE_NAME, VERSION)
        .env_prefix("REVOLUT")
        .crate_target(CRATE_TARGET)
        .services(SERVICES)
        .sign_in(SignIn::OwnerPassword)
        .icon(
            include_bytes!("../assets/favicon.png"),
            include_str!("../assets/favicon.svg"),
        )
        .docs_url("https://github.com/Trozz/mcp-revolut-business#configuration")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_server_core::service::ServiceEnv;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> ServiceEnv {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        ServiceEnv::new(Box::new(move |k: &str| map.get(k).cloned()))
    }

    const KEY: &str =
        include_str!("../../mcp-revolut-business-banking/tests/fixtures/test-ed25519.pem");

    fn core() -> Vec<(&'static str, &'static str)> {
        vec![
            ("PUBLIC_URL", "https://bank-mcp.example.com"),
            ("OAUTH_SIGNING_KEY", "0123456789abcdef0123456789abcdef"),
            ("OAUTH_ADMIN_PASSWORD", "a long test password"),
            (
                "REVOLUT_API_KEY",
                "r_test_placeholder0000000000000000000000000000000000000000000000",
            ),
            ("REVOLUT_PRIVATE_KEY", KEY),
        ]
    }

    #[test]
    fn the_owner_signs_in_with_a_password() {
        let mut e = env(&core());
        let loaded = server().load(&mut e).expect("loads");
        assert!(e.errors.is_empty(), "{:?}", e.errors);
        assert!(!loaded.verifier.carried());
        assert!(loaded.verifier.upstream().is_none());
    }

    #[test]
    fn the_keys_are_required_and_errors_are_reported_together() {
        let mut e = env(&[("PUBLIC_URL", "ftp://x")]);
        assert!(server().load(&mut e).is_none());
        let joined = e.errors.join("\n");
        for needle in [
            "PUBLIC_URL",
            "OAUTH_SIGNING_KEY is required",
            "REVOLUT_API_KEY is required",
            "REVOLUT_PRIVATE_KEY is required",
        ] {
            assert!(joined.contains(needle), "{needle}: {joined}");
        }
    }

    #[test]
    fn a_key_that_is_not_ed25519_is_reported() {
        let mut vars = core();
        vars.retain(|(k, _)| *k != "REVOLUT_PRIVATE_KEY");
        vars.push((
            "REVOLUT_PRIVATE_KEY",
            "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----",
        ));
        let mut e = env(&vars);
        assert!(server().load(&mut e).is_none());
        assert!(e.errors.join("\n").contains("Ed25519"), "{:?}", e.errors);
    }

    fn names(extra: &[(&'static str, &'static str)]) -> Vec<String> {
        let mut e = env(&[core(), extra.to_vec()].concat());
        let loaded = server().load(&mut e).expect("loads");
        Server::combined_tool_names(&loaded.services)
    }

    #[test]
    fn payment_tools_appear_only_when_allowed() {
        let off = names(&[]);
        assert!(off.contains(&"revolut_create_payment_draft".to_string()));
        assert!(!off.iter().any(|n| n == "revolut_create_payment"));
        let on = names(&[("REVOLUT_ALLOW_PAYMENTS", "true")]);
        for t in [
            "create_payment",
            "transfer_between_accounts",
            "exchange_currency",
        ] {
            assert!(on.contains(&format!("revolut_{t}")), "{t}");
        }
    }

    #[test]
    fn read_only_hides_the_write_tools() {
        let mut e = env(&[core(), vec![("REVOLUT_READ_ONLY", "revolut")]].concat());
        let loaded = server().load(&mut e).expect("loads");
        assert!(loaded.safety.is_read_only("revolut"));
    }

    #[test]
    fn combined_tool_names_are_unique_and_fit_clients() {
        let names = names(&[("REVOLUT_ALLOW_PAYMENTS", "true")]);
        assert!(names.len() >= 30, "{}", names.len());
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len());
        for n in &names {
            assert!(
                n.len() <= 64
                    && n.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "{n}"
            );
        }
    }

    #[test]
    fn debug_directive_covers_service_crates() {
        use tracing::Level;
        use tracing_subscriber::layer::SubscriberExt;
        let directives = mcp_server_core::observability::build_filter("info", true, CRATE_TARGET);
        let subscriber =
            tracing_subscriber::registry().with(tracing_subscriber::EnvFilter::new(directives));
        tracing::subscriber::with_default(subscriber, || {
            assert!(tracing::enabled!(target: "mcp_revolut_business_banking::api", Level::DEBUG));
            assert!(!tracing::enabled!(target: "hyper", Level::DEBUG));
        });
    }
}
