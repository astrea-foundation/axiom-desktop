use std::io::Write;
use tokio::io::AsyncReadExt;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let result = async {
        if args == ["--self-test"] {
            axiom_direct_verifier::self_test()?;
            return Ok(serde_json::json!({"passed":true,"native":true}));
        }
        anyhow::ensure!(args.is_empty(), "invalid arguments");
        let mut input = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(55),
            tokio::io::stdin()
                .take(axiom_direct_verifier::MAX_INPUT as u64 + 1)
                .read_to_end(&mut input),
        )
        .await??;
        axiom_direct_verifier::verify(&input).await
    }
    .await;
    if let Ok(value) = result {
        let out = serde_json::to_vec(&value).unwrap_or_default();
        if out.len() <= 64 * 1024 && !out.is_empty() && std::io::stdout().write_all(&out).is_ok() {
            return;
        }
    }
    eprintln!("direct workload attestation rejected");
    std::process::exit(1);
}
