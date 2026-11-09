#[derive(Debug, thiserror::Error)]
pub enum RvError {
    /// Revolut refused the server's API key or its signature: the key was revoked or expired, or
    /// the private key is not the one registered with it. Only the server's operator can fix it.
    #[error(
        "Revolut Business refused this server's API key ({0}). The server's operator needs to check REVOLUT_API_KEY and REVOLUT_PRIVATE_KEY, and that the key is still active in Revolut Business (Settings, APIs)"
    )]
    Auth(String),
    /// Revolut refused: the API key's access or its IP allowlist does not allow it.
    #[error("{0}")]
    Permission(String),
    /// A complete message: tool errors are already prefixed with the kind, `not_found: `.
    #[error("{0}")]
    NotFound(String),
    #[error("{}", rate_limited_message(*.retry_after_secs))]
    RateLimited {
        /// Seconds Revolut asked us to wait, when it said.
        retry_after_secs: Option<u64>,
    },
    #[error("{0}")]
    Validation(String),
    /// A payment tool was called on a server that does not allow payments.
    #[error("{0}")]
    Disabled(String),
    #[error("Revolut Business error: {0}")]
    Upstream(String),
    #[error("{0}")]
    Internal(String),
}

fn rate_limited_message(retry_after_secs: Option<u64>) -> String {
    let base = "the Revolut Business API rate limit is used up";
    match retry_after_secs {
        Some(s) => format!("{base}; try again in {s} seconds"),
        None => format!("{base}; try again in a minute"),
    }
}

impl RvError {
    pub fn kind(&self) -> &'static str {
        match self {
            RvError::Auth(_) => "auth",
            RvError::Permission(_) => "permission",
            RvError::NotFound(_) => "not_found",
            RvError::RateLimited { .. } => "rate_limited",
            RvError::Validation(_) => "validation",
            RvError::Disabled(_) => "disabled",
            RvError::Upstream(_) => "upstream",
            RvError::Internal(_) => "internal",
        }
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        RvError::Validation(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, RvError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        for (e, kind) in [
            (RvError::Auth("x".into()), "auth"),
            (RvError::Permission("x".into()), "permission"),
            (RvError::NotFound("x".into()), "not_found"),
            (
                RvError::RateLimited {
                    retry_after_secs: None,
                },
                "rate_limited",
            ),
            (RvError::invalid("x"), "validation"),
            (RvError::Disabled("x".into()), "disabled"),
            (RvError::Upstream("x".into()), "upstream"),
            (RvError::Internal("x".into()), "internal"),
        ] {
            assert_eq!(e.kind(), kind);
        }
    }

    #[test]
    fn messages_say_what_to_do() {
        assert!(
            RvError::Auth("x".into())
                .to_string()
                .contains("REVOLUT_API_KEY")
        );
        let m = RvError::RateLimited {
            retry_after_secs: Some(30),
        }
        .to_string();
        assert!(m.contains("30 seconds"), "{m}");
    }
}
