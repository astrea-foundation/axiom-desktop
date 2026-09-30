fn main() {
    println!("cargo:rerun-if-env-changed=AXIOM_UPDATE_PUBLIC_KEYS");
    println!("cargo:rerun-if-env-changed=AXIOM_SIGNING_PUBLISHER");
    println!("cargo:rerun-if-env-changed=AXIOM_SETUP_NOTICES_FILE");
    let notices = if let Some(path) = std::env::var_os("AXIOM_SETUP_NOTICES_FILE") {
        println!(
            "cargo:rerun-if-changed={}",
            std::path::Path::new(&path).display()
        );
        println!("cargo:rustc-env=AXIOM_SETUP_LICENSES_INCLUDED=true");
        let text = std::fs::read_to_string(path).expect("Read bundled setup notices");
        assert!(
            text.contains("native-windows-gui-1.0.13/"),
            "Setup notices must include the Windows GUI license"
        );
        text
    } else {
        assert!(
            std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
                || std::env::var("AXIOM_UPDATE_PUBLIC_KEYS")
                    .unwrap_or_default()
                    .is_empty(),
            "Stable setup builds require AXIOM_SETUP_NOTICES_FILE; use scripts/package-setup.mjs"
        );
        println!("cargo:rerun-if-changed=../../LICENSE");
        println!("cargo:rustc-env=AXIOM_SETUP_LICENSES_INCLUDED=false");
        std::fs::read_to_string("../../LICENSE").expect("Read project license")
    };
    std::fs::write(
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("setup-licenses.txt"),
        notices,
    )
    .expect("Embed setup notices");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../desktop/build/icon.ico")
            .set("ProductName", "Axiom Setup")
            .set("FileDescription", "Install the latest Axiom")
            .set("CompanyName", "Astrea Labs, Inc.")
            .set_manifest(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
<assemblyIdentity version="1.0.0.0" processorArchitecture="*" name="stream.axiom.setup" type="win32"/>
<dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency>
<trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges><requestedExecutionLevel level="asInvoker" uiAccess="false"/></requestedPrivileges></security></trustInfo>
<compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1"><application><supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/></application></compatibility>
</assembly>"#);
        resource.compile().expect("Compile Axiom Setup resources");
    }
}
