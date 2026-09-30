#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod gui;
#[cfg(windows)]
mod windows;

fn main() {
    #[cfg(windows)]
    if std::env::args().any(|arg| arg == "--verify-build") {
        // Used by native Windows qualification; no network or installation.
        let result = (|| -> anyhow::Result<()> {
            gui::run(true)?;
            let arguments = std::env::args().collect::<Vec<_>>();
            let path = arguments
                .windows(2)
                .find(|pair| pair[0] == "--metadata-output")
                .ok_or_else(|| anyhow::anyhow!("Metadata output is required"))?;
            std::fs::write(
                &path[1],
                serde_json::to_vec(&serde_json::json!({"version":env!("CARGO_PKG_VERSION"),
                "trustedKeys":axiom_setup::TRUSTED_KEYS,"publisher":axiom_setup::PUBLISHER,
                "licensesIncluded":axiom_setup::LICENSES_INCLUDED == "true"}))?,
            )?;
            Ok(())
        })();
        if result.is_err() {
            std::process::exit(1);
        }
    } else if let Err(error) = gui::run(false) {
        native_windows_gui::simple_message(
            "Axiom Setup",
            &format!("Couldn’t start setup: {error}"),
        );
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    {
        eprintln!("Axiom Setup is available for Windows only.");
        std::process::exit(1);
    }
}
