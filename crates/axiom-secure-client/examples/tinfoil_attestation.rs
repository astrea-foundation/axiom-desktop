//! Verify the public Tinfoil router without sending a prompt or API key.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = tinfoil::SecureClient::new(
        "inference.tinfoil.sh",
        "tinfoilsh/confidential-model-router",
        "",
    );
    let verified =
        tokio::time::timeout(std::time::Duration::from_secs(90), client.verify()).await??;
    println!("{}", serde_json::to_string_pretty(&verified)?);
    Ok(())
}
