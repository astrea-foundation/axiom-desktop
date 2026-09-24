//! One paid-upstream search through the authenticated hosted API, with no model
//! inference or credential output. Uses the existing native account session.
use std::time::Duration;

use axiomcli::{
    auth::{AuthManager, ValidationStatus},
    web::{HostedSearchClient, SearchProvider as _},
};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let origin = std::env::var("AXIOM_BASE_URL")?;
    let auth = AuthManager::new(&origin, Duration::from_secs(20))?;
    anyhow::ensure!(
        matches!(auth.validate().await, ValidationStatus::Valid(_)),
        "Sign in to this Axiom environment first"
    );
    let search = HostedSearchClient::new(auth)?;
    let report = search
        .search("Zcash official documentation", CancellationToken::new())
        .await?;
    anyhow::ensure!(!report.results.is_empty(), "Search returned no results");
    println!(
        "Authenticated hosted search succeeded: {} results, {} warnings. No credentials or query results logged.",
        report.results.len(),
        report.warnings.len()
    );
    Ok(())
}
