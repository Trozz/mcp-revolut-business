use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    mcp_revolut_business::server().main().await
}
