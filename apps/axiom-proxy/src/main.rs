#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    axiom_proxy::run_standalone().await
}
