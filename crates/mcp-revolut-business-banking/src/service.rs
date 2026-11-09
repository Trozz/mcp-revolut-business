use crate::api::{ApiSettings, RvClient, http_client};
use crate::config::BankingConfig;
use crate::signing::Signer;
use crate::tools::BankingServer;
use mcp_server_core::combined::DynHandler;
use mcp_server_core::service::{Service, ServiceContext, ServiceEnv, mcp_service};
use secrecy::ExposeSecret;
use std::sync::Arc;

/// The Revolut Business service, mounted by the server at `/revolut`.
pub struct BankingService {
    client: Arc<RvClient>,
    allow_payments: bool,
    sandbox: bool,
}

impl BankingService {
    pub fn new(client: Arc<RvClient>, allow_payments: bool, sandbox: bool) -> Self {
        Self {
            client,
            allow_payments,
            sandbox,
        }
    }

    fn server(&self) -> BankingServer {
        BankingServer::new(self.client.clone(), self.allow_payments)
    }
}

impl Service for BankingService {
    fn name(&self) -> &'static str {
        "revolut"
    }

    fn tool_names(&self) -> Vec<String> {
        self.server().tool_names()
    }

    fn router(self: Arc<Self>, ctx: &ServiceContext) -> axum::Router {
        tracing::info!(
            api = %self.client.base(),
            sandbox = self.sandbox,
            payments = self.allow_payments,
            "revolut settings"
        );
        mcp_service(ctx, move || Ok(self.server()))
    }

    fn handler(&self) -> DynHandler {
        DynHandler::new(self.server())
    }
}

/// Builds the Revolut client: the API key, its signing key and the API.
pub fn client(cfg: &BankingConfig) -> Result<RvClient, String> {
    let signer = Signer::from_pem(cfg.private_key.expose_secret())
        .map_err(|e| format!("{}: {e}", crate::config::PRIVATE_KEY_VAR))?;
    let settings = ApiSettings::new(cfg.api_url.clone());
    let http = http_client(&settings).map_err(|e| e.to_string())?;
    Ok(RvClient::new(http, settings, cfg.api_key.clone(), signer))
}

/// Reads the Revolut configuration and builds the service. Errors go to `env.errors`.
pub fn from_env(env: &mut ServiceEnv) -> Option<Arc<dyn Service>> {
    let cfg = BankingConfig::from_env(env)?;
    match client(&cfg) {
        Ok(c) => Some(Arc::new(BankingService::new(
            Arc::new(c),
            cfg.allow_payments,
            cfg.sandbox,
        ))),
        Err(e) => {
            env.errors.push(e);
            None
        }
    }
}
