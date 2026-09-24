use std::process::Command;

#[test]
fn exec_uses_the_shared_runtime_and_persists_a_terminal_turn() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("isolated state");
    let binary = env!("CARGO_BIN_EXE_axiomcli");
    let configure = |command: &mut Command| {
        command
            .env("AXIOMCLI_TEST_RUNNER", "echo")
            .env("XDG_CONFIG_HOME", state.path().join("config"))
            .env("XDG_DATA_HOME", state.path().join("data"))
            .current_dir(workspace.path());
    };

    let mut exec = Command::new(binary);
    configure(&mut exec);
    let output = exec
        .args(["exec", "persist this turn"])
        .output()
        .expect("headless exec");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("AxiomCLI received: persist this turn")
    );

    let mut list = Command::new(binary);
    configure(&mut list);
    let output = list
        .args(["sessions", "list", "--all"])
        .output()
        .expect("list sessions");
    assert!(output.status.success());
    let listing = String::from_utf8(output.stdout).expect("listing");
    let id = listing.split_whitespace().next().expect("session ID");

    let mut show = Command::new(binary);
    configure(&mut show);
    let output = show
        .args(["sessions", "show", id])
        .output()
        .expect("show session");
    assert!(output.status.success());
    let state: serde_json::Value = serde_json::from_slice(&output.stdout).expect("state JSON");
    assert_eq!(state["thread"]["origin"], "headless");
    assert_eq!(state["thread"]["lifecycle"], "ready");
    assert!(
        state["turns"]
            .as_array()
            .expect("turns")
            .iter()
            .any(|turn| turn["status"] == "completed")
    );
    let timeline = state["timeline_items"].as_array().expect("timeline items");
    assert!(timeline.iter().any(|item| item["kind"] == "user_message"));
    assert!(timeline.iter().any(|item| {
        item["kind"] == "assistant_message"
            && item["content"] == "AxiomCLI received: persist this turn"
    }));
}

#[test]
fn system_prompt_file_is_global_and_validated_before_launch() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("isolated state");
    let prompt_file = workspace.path().join("agent-instructions.md");
    std::fs::write(&prompt_file, "Be concise and verify every change.\n").expect("write prompt");

    let output = Command::new(env!("CARGO_BIN_EXE_axiomcli"))
        .env("AXIOMCLI_TEST_RUNNER", "echo")
        .env("XDG_CONFIG_HOME", state.path().join("config"))
        .env("XDG_DATA_HOME", state.path().join("data"))
        .current_dir(workspace.path())
        .args([
            "exec",
            "use the launch instructions",
            "--system-prompt-file",
        ])
        .arg(&prompt_file)
        .output()
        .expect("headless exec with prompt file");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let missing = workspace.path().join("missing-instructions.md");
    let output = Command::new(env!("CARGO_BIN_EXE_axiomcli"))
        .env("AXIOMCLI_TEST_RUNNER", "echo")
        .env("XDG_CONFIG_HOME", state.path().join("config"))
        .env("XDG_DATA_HOME", state.path().join("data"))
        .current_dir(workspace.path())
        .arg("--system-prompt-file")
        .arg(&missing)
        .args(["exec", "this must not run"])
        .output()
        .expect("missing prompt file rejection");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed to open system prompt file"));
}
