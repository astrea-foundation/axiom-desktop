#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _installation_lease = axiom_installation::lease()?;
    axiom_proxy::run_standalone().await
}
