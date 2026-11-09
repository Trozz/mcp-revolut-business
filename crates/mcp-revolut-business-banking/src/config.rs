//! The Revolut Business service's own settings: the API key and the private key it signs requests
//! with, which Revolut it talks to, and whether it may move money. The server binary owns the core
//! configuration (URL, OAuth signing key, owner password, TLS, logging).

use mcp_server_core::config::EnvReader;
use secrecy::{ExposeSecret, SecretString};
use url::Url;

/// MCP `serverInfo.name` of the Revolut Business service.
pub const SERVICE_NAME: &str = "mcp-revolut-business-banking";

pub const PRODUCTION_API_URL: &str = "https://b2b.revolut.com/api/1.0";
pub const SANDBOX_API_URL: &str = "https://sandbox-b2b.revolut.com/api/1.0";

pub const API_KEY_VAR: &str = "REVOLUT_API_KEY";
pub const PRIVATE_KEY_VAR: &str = "REVOLUT_PRIVATE_KEY";
pub const ENVIRONMENT_VAR: &str = "REVOLUT_ENVIRONMENT";
pub const API_URL_VAR: &str = "REVOLUT_API_URL";
pub const ALLOW_PAYMENTS_VAR: &str = "REVOLUT_ALLOW_PAYMENTS";

pub struct BankingConfig {
    /// The API key, `r_live_...` or `r_test_...`.
    pub api_key: SecretString,
    /// The Ed25519 private key registered with the API key, PEM.
    pub private_key: SecretString,
    /// Whether this is Revolut's sandbox.
    pub sandbox: bool,
    /// The API base, ending in `/api/1.0`.
    pub api_url: Url,
    /// Whether payment tools (pay, transfer, exchange) may run. Off by default: payment drafts,
    /// which someone approves in the Revolut Business app, work either way.
    pub allow_payments: bool,
}

impl BankingConfig {
    pub fn from_env<F: Fn(&str) -> Option<String>>(env: &mut EnvReader<F>) -> Option<Self> {
        let errors_before = env.errors.len();
        let api_key = env.secret_required(API_KEY_VAR);
        let key_sandbox = api_key.as_ref().and_then(|k| {
            let k = k.expose_secret().trim();
            let plausible = k.len() <= 200
                && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
            if !plausible {
                env.errors.push(format!(
                    "{API_KEY_VAR} must be the API key from Revolut Business (Settings, APIs), which starts r_live_ or r_test_"
                ));
                None
            } else if k.starts_with("r_test_") {
                Some(true)
            } else if k.starts_with("r_live_") {
                Some(false)
            } else {
                None
            }
        });
        let private_key = env.secret_required(PRIVATE_KEY_VAR);
        let environment = env.optional(ENVIRONMENT_VAR);
        let sandbox = match environment.as_deref().map(str::trim) {
            None | Some("") => key_sandbox.unwrap_or(false),
            Some("production") | Some("live") => false,
            Some("sandbox") => true,
            Some(other) => {
                env.errors.push(format!(
                    "{ENVIRONMENT_VAR} must be production or sandbox, got {other:?}"
                ));
                false
            }
        };
        if let Some(k) = key_sandbox
            && k != sandbox
        {
            env.errors.push(format!(
                "{API_KEY_VAR} is a {} key but {ENVIRONMENT_VAR} is {}",
                if k {
                    "sandbox (r_test_)"
                } else {
                    "live (r_live_)"
                },
                if sandbox { "sandbox" } else { "production" }
            ));
        }
        let default = if sandbox {
            SANDBOX_API_URL
        } else {
            PRODUCTION_API_URL
        };
        let raw = env
            .optional(API_URL_VAR)
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| default.to_string());
        let api_url = match parse_url(API_URL_VAR, &raw) {
            Ok(u) => Some(u),
            Err(msg) => {
                env.errors.push(msg);
                None
            }
        };
        let allow_payments = env.bool(ALLOW_PAYMENTS_VAR, false);
        if env.errors.len() != errors_before {
            return None;
        }
        Some(BankingConfig {
            api_key: api_key?.expose_secret().trim().to_string().into(),
            private_key: private_key?,
            sandbox,
            api_url: api_url?,
            allow_payments,
        })
    }
}

/// A URL from variable `name`: https, or http to a loopback address; no credentials, query or
/// fragment. A trailing slash is removed.
pub fn parse_url(name: &str, raw: &str) -> Result<Url, String> {
    let mut url = Url::parse(raw.trim()).map_err(|e| format!("{name} is not a valid URL: {e}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{name} must not contain credentials"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(format!("{name} must not contain a query or fragment"));
    }
    let ok = (url.scheme() == "https" && url.host_str().is_some())
        || (url.scheme() == "http" && is_loopback(&url));
    if !ok {
        return Err(format!(
            "{name} must use https (http is only allowed for loopback addresses)"
        ));
    }
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    Ok(url)
}

pub(crate) fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const LIVE: &[(&str, &str)] = &[
        (API_KEY_VAR, "r_live_abc123"),
        (PRIVATE_KEY_VAR, "-----BEGIN PRIVATE KEY-----"),
    ];
    const TEST: &[(&str, &str)] = &[
        (API_KEY_VAR, "r_test_abc123"),
        (PRIVATE_KEY_VAR, "-----BEGIN PRIVATE KEY-----"),
    ];

    fn load(pairs: &[(&str, &str)]) -> (Option<BankingConfig>, Vec<String>) {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let mut env = EnvReader::new(move |k: &str| map.get(k).cloned());
        let c = BankingConfig::from_env(&mut env);
        (c, env.errors)
    }

    #[test]
    fn the_key_chooses_live_or_sandbox_and_payments_are_off() {
        let (c, errors) = load(LIVE);
        assert!(errors.is_empty(), "{errors:?}");
        let c = c.unwrap();
        assert!(!c.sandbox);
        assert_eq!(c.api_url.as_str(), PRODUCTION_API_URL);
        assert!(!c.allow_payments);
        let (c, errors) = load(TEST);
        assert!(errors.is_empty(), "{errors:?}");
        let c = c.unwrap();
        assert!(c.sandbox);
        assert_eq!(c.api_url.as_str(), SANDBOX_API_URL);
    }

    #[test]
    fn overrides() {
        let (c, errors) = load(
            &[
                TEST,
                &[
                    (ENVIRONMENT_VAR, "sandbox"),
                    (API_URL_VAR, "http://127.0.0.1:9999/api/1.0/"),
                    (ALLOW_PAYMENTS_VAR, "true"),
                ],
            ]
            .concat(),
        );
        assert!(errors.is_empty(), "{errors:?}");
        let c = c.unwrap();
        assert_eq!(c.api_url.as_str(), "http://127.0.0.1:9999/api/1.0");
        assert!(c.allow_payments);
    }

    #[test]
    fn a_key_for_the_other_environment_is_refused() {
        let (_, errors) = load(&[TEST, &[(ENVIRONMENT_VAR, "production")]].concat());
        assert!(
            errors.join("\n").contains("sandbox (r_test_) key"),
            "{errors:?}"
        );
        let (_, errors) = load(&[LIVE, &[(ENVIRONMENT_VAR, "sandbox")]].concat());
        assert!(
            errors.join("\n").contains("live (r_live_) key"),
            "{errors:?}"
        );
    }

    #[test]
    fn every_problem_is_reported() {
        let (c, errors) = load(&[
            (API_KEY_VAR, "not a key!"),
            (ENVIRONMENT_VAR, "staging"),
            (API_URL_VAR, "http://b2b.revolut.com/api/1.0"),
        ]);
        assert!(c.is_none());
        let joined = errors.join("\n");
        for needle in [
            "starts r_live_ or r_test_",
            "REVOLUT_PRIVATE_KEY is required",
            "production or sandbox",
            "must use https",
        ] {
            assert!(joined.contains(needle), "{needle}: {joined}");
        }
    }
}
