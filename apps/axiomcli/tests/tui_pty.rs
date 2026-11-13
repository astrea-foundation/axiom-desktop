#![cfg(unix)]

use std::{
    io::{Read as _, Write as _},
    path::Path,
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use axiomcli::{
    app::{AppCommand, Origin, PermissionProfile, Runtime, SessionId, ThinkingLevel},
    session::{SessionStore, model_for_resume, thinking_for_resume},
};

const TEST_ACCOUNT_ID: &str = "local-test-account";

fn test_account_database(state_path: &Path) -> std::path::PathBuf {
    state_path
        .join("data")
        .join("axiom")
        .join("accounts")
        .join(TEST_ACCOUNT_ID)
        .join("cli")
        .join("state.sqlite3")
}

fn test_cli_command(state_path: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_axiomcli"));
    command
        .env("AXIOMCLI_TEST_RUNNER", "echo")
        .env("XDG_CONFIG_HOME", state_path.join("config"))
        .env("XDG_DATA_HOME", state_path.join("data"));
    command
}

fn seed_tui_session_settings(
    store: &SessionStore,
    cwd: &Path,
    model: &str,
    thinking: ThinkingLevel,
) -> SessionId {
    tokio::runtime::Runtime::new()
        .expect("test runtime")
        .block_on(async {
            let runtime = Runtime::new(32);
            let session_id = runtime.session_id();
            let created = runtime
                .dispatch(AppCommand::CreateSession {
                    session_id: session_id.clone(),
                    cwd: cwd.to_path_buf(),
                    origin: Origin::Tui,
                    profile: PermissionProfile::Confirm,
                })
                .await
                .expect("create seeded TUI session");
            store.append_all(&created).expect("persist TUI session");
            let settings = runtime
                .dispatch(AppCommand::ChangeModelSettings {
                    session_id: session_id.clone(),
                    model: model.into(),
                    thinking,
                    reset_security: false,
                })
                .await
                .expect("seed TUI model settings");
            store
                .append_all(&settings)
                .expect("persist TUI model settings");
            session_id
        })
}

fn run_tui(runner: &str, panic_probe: bool) -> (String, bool) {
    let state = tempfile::tempdir().expect("isolated TUI state");
    run_tui_at(runner, panic_probe, state.path(), &[])
}

fn run_tui_at(
    runner: &str,
    panic_probe: bool,
    state_path: &Path,
    extra_args: &[&str],
) -> (String, bool) {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 14,
            cols: 60,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open PTY");
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_axiomcli"));
    command.arg("tui");
    for argument in extra_args {
        command.arg(argument);
    }
    let test_runner = if matches!(
        runner,
        "resume"
            | "resume_reconcile"
            | "remembered_model"
            | "preference_bootstrap"
            | "delete"
            | "composer"
            | "appearance"
            | "gift"
            | "usage"
            | "paste_picker"
    ) {
        "echo"
    } else if runner == "input_active" {
        "blocking"
    } else {
        runner
    };
    command.env("AXIOMCLI_TEST_RUNNER", test_runner);
    command.env("AXIOMCLI_ASCII", "1");
    // Native credentials are scoped to the API origin, not the XDG directory.
    // Use a fresh loopback origin so this production-runner test cannot discover
    // the developer's signed-in session or contact the production service.
    let signed_out_service = (runner == "signed_out").then(|| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("isolated origin");
        listener.set_nonblocking(true).unwrap();
        listener
    });
    if runner == "signed_out" {
        command.env_remove("AXIOMCLI_TEST_RUNNER");
        command.env_remove("AXIOM_API_KEY");
        command.env_remove("AXIOMCLI_CREDENTIAL_STORE");
        let origin = format!(
            "https://{}",
            signed_out_service.as_ref().unwrap().local_addr().unwrap()
        );
        command.env("AXIOM_BASE_URL", &origin);
        command.env("AXIOM_AUTH_URL", &origin);
    }
    command.env("NO_COLOR", "1");
    if runner == "appearance" {
        command.env("AXIOMCLI_COLOR", "truecolor");
    }
    command.env("XDG_CONFIG_HOME", state_path.join("config"));
    command.env("XDG_DATA_HOME", state_path.join("data"));
    if runner == "workspace_edit" {
        command.env("AXIOM_PERMISSION_PROFILE", "full_access");
    }
    if panic_probe {
        command.env("AXIOMCLI_TEST_TUI_PANIC", "1");
    }
    let mut child = pair.slave.spawn_command(command).expect("spawn TUI in PTY");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("PTY reader");
    let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>();
    let reader_thread = thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    if output_tx.send(buffer[..read].to_vec()).is_err() {
                        break;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    });
    let mut output = Vec::new();

    let mut writer = pair.master.take_writer().expect("PTY writer");
    // Crossterm asks the terminal for its cursor position while initializing.
    // A real terminal answers this DSR request; the PTY harness must emulate it.
    wait_for_output(
        &output_rx,
        &mut output,
        b"\x1b[6n",
        "cursor position request",
    );
    writer
        .write_all(b"\x1b[1;1R")
        .expect("cursor position response");
    writer.flush().expect("flush cursor response");
    if !panic_probe {
        // Wait for the first frame's cursor visibility command before resizing.
        // Resizing during initialization can make the first frame already wide,
        // which does not acknowledge that the input loop consumed the resize.
        wait_for_output(
            &output_rx,
            &mut output,
            b"\x1b[?25",
            "initial interactive frame",
        );
    }
    pair.master
        .resize(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("resize PTY");
    if !panic_probe {
        // The expanded footer appears after the 100-column resize,
        // proving the input loop is active.
        wait_for_output(
            &output_rx,
            &mut output,
            if runner == "signed_out" {
                b"Sign in"
            } else {
                b"help"
            },
            "resized interactive screen",
        );
        let completed_before_probe = completed_turns(state_path);
        if runner != "signed_out" {
            writer.write_all(b"probe\r").expect("submit prompt");
            writer.flush().expect("flush prompt");
            if test_runner == "echo" {
                wait_for_completed_turn(
                    state_path,
                    &completed_before_probe,
                    &output_rx,
                    &mut output,
                );
            }
        }
        if runner == "input_active" {
            wait_for_output(&output_rx, &mut output, b"Working", "active task");
            writer
                .write_all(b"/usage\r")
                .expect("inspect usage while running");
            writer.flush().unwrap();
            wait_for_output(
                &output_rx,
                &mut output,
                b"Usage",
                "usage inspector during task",
            );
            writer
                .write_all(b"\x1b")
                .expect("close usage without stopping");
            writer.flush().unwrap();
            thread::sleep(Duration::from_millis(100));
            writer
                .write_all(b"/logout\r")
                .expect("reject session mutation while running");
            writer.flush().unwrap();
            wait_for_output(&output_rx, &mut output, b"Stop", "active command guard");
            writer
                .write_all(&[127; 7])
                .expect("clear preserved logout draft");
            writer
                .write_all(b"aBcD\x1b[200~ PaStEd\nFollowUp\x1b[201~")
                .expect("type and paste steering draft");
            writer.flush().unwrap();
            writer.write_all(b"\x1b").expect("stop task and keep draft");
            writer.flush().unwrap();
            wait_for_output(&output_rx, &mut output, b"Cancelled", "cancel active task");
            writer
                .write_all(b"\r")
                .expect("submit preserved draft after cancellation");
            writer.flush().unwrap();
            thread::sleep(Duration::from_millis(250));
        } else if runner == "paste_picker" {
            writer.write_all(b"/model\r").expect("open model picker");
            writer.flush().unwrap();
            wait_for_output(&output_rx, &mut output, b"Select model", "model picker");
            wait_for_output(
                &output_rx,
                &mut output,
                b"medium-only",
                "loaded model catalog",
            );
            writer
                .write_all(b"\x1b[200~alternate-model\x1b[201~\r")
                .expect("paste filter and select model");
            writer.flush().unwrap();
            wait_for_output(
                &output_rx,
                &mut output,
                b"Model changed",
                "pasted model filter",
            );
            let completed_before = completed_turns(state_path);
            writer
                .write_all(b"AfterPicker\r")
                .expect("submit clean draft");
            writer.flush().unwrap();
            wait_for_completed_turn(state_path, &completed_before, &output_rx, &mut output);
        } else if runner == "blocking" {
            wait_for_output(&output_rx, &mut output, b"Working", "blocking turn");
        } else if runner == "questions" {
            wait_for_output(
                &output_rx,
                &mut output,
                b"Axiom needs your input",
                "first question",
            );
            writer.write_all(b"1\r").expect("first answer");
            writer.flush().expect("flush first answer");
            writer.write_all(b"compact\r").expect("second answer");
            writer.flush().expect("flush second answer");
            thread::sleep(Duration::from_millis(180));
        } else if runner == "approval" {
            wait_for_output(
                &output_rx,
                &mut output,
                b"Permission required",
                "approval card",
            );
            // An approval can arrive in the middle of ordinary typing. Neither
            // lowercase, uppercase nor pasted letters may authorize or deny it.
            writer
                .write_all(b"negpyNEGPY\x05\x1b[200~yNeP\x1b[201~\r")
                .expect("ordinary typing during approval");
            writer.flush().expect("flush ordinary typing");
            thread::sleep(Duration::from_millis(200));
            while let Ok(chunk) = output_rx.try_recv() {
                output.extend(chunk);
            }
            let before_decision = String::from_utf8_lossy(&output);
            assert!(
                !before_decision.contains("allow_once"),
                "typing authorized the tool: {before_decision}"
            );
            assert!(
                !before_decision.contains("Cancelled"),
                "typing cancelled the tool: {before_decision}"
            );
            writer.write_all(&[25]).expect("Ctrl+Y allows once");
            writer.flush().expect("flush approval");
            wait_for_output(&output_rx, &mut output, b"allow_once", "approval result");
        } else if runner == "plan_review" {
            wait_for_output(
                &output_rx,
                &mut output,
                b"Axiom needs your input",
                "review decision",
            );
            writer.write_all(b"2\r").expect("revision decision");
            writer.flush().expect("flush revision decision");
            writer.write_all(b"4\r").expect("revision line");
            writer.flush().expect("flush revision line");
            writer
                .write_all(b"Add a verification step\r")
                .expect("revision comment");
            writer.flush().expect("flush revision comment");
            thread::sleep(Duration::from_millis(180));
        } else if runner == "usage" {
            writer.write_all(b"/usage\r").expect("open usage");
            writer.flush().expect("flush usage command");
            wait_for_output(&output_rx, &mut output, b"Usage", "usage inspector");
            writer.write_all(b"\x1b").expect("close usage");
            writer.flush().expect("flush close");
        } else if runner == "echo" {
            writer
                // Open the permission window on Confirm, move to Full Access,
                // then apply the selected tool profile.
                .write_all(b"/permissions\r\x1b[B\r")
                .expect("arrow-select and change permission profile");
            writer.flush().expect("flush slash command");
            wait_for_output(
                &output_rx,
                &mut output,
                b"Full Access",
                "slash permission result",
            );
            writer.write_all(b"/model\r").expect("open model picker");
            writer.flush().expect("flush model picker command");
            wait_for_output(&output_rx, &mut output, b"Select model", "model picker");
            wait_for_output(
                &output_rx,
                &mut output,
                b"medium-only",
                "loaded model catalog",
            );
            writer
                // The default model is the second deterministic catalog
                // entry, so Arrow Up selects alternate-model.
                .write_all(b"\x1b[A\r")
                .expect("move and select model");
            writer.flush().expect("flush model selection");
            wait_for_output(
                &output_rx,
                &mut output,
                b"Model changed",
                "selected model result",
            );
            // Move to transcript focus, then prove `/` returns to the composer.
            // The first Enter completes the invalid prefix as if Tab were
            // pressed; the second executes the completed command.
            writer
                .write_all(b"\t/thinking h\r\r")
                .expect("focus slash composer and complete thinking");
            writer.flush().expect("flush thinking command");
            wait_for_output(
                &output_rx,
                &mut output,
                b"Thinking set to high",
                "slash thinking result",
            );
            let completed_before = completed_turns(state_path);
            writer
                .write_all(b"remember this model\r")
                .expect("submit prompt with selected model");
            writer.flush().expect("flush selected-model prompt");
            wait_for_completed_turn(state_path, &completed_before, &output_rx, &mut output);
        } else if runner == "delete" {
            writer.write_all(b"/delete\r").expect("open delete picker");
            writer.flush().expect("flush delete command");
            wait_for_output(
                &output_rx,
                &mut output,
                b"Delete transcripts",
                "delete picker",
            );
            writer
                .write_all(&[1])
                .expect("select all visible transcripts");
            writer.flush().expect("flush select all");
            writer
                .write_all(b"\r")
                .expect("continue to deletion confirmation");
            writer.flush().expect("flush deletion confirmation");
            wait_for_output(
                &output_rx,
                &mut output,
                b"Permanently delete",
                "permanent deletion warning",
            );
            writer.write_all(b"y").expect("confirm transcript deletion");
            writer.flush().expect("flush permanent deletion");
            wait_for_output(&output_rx, &mut output, b"Deleted", "deletion result");
        } else if runner == "resume" {
            writer.write_all(b"/resume\r").expect("open resume picker");
            writer.flush().expect("flush resume command");
            wait_for_output(
                &output_rx,
                &mut output,
                b"Resume transcript",
                "resume picker",
            );
            writer
                .write_all(b"Earlier cherry\r")
                .expect("search and select transcript");
            writer.flush().expect("flush transcript selection");
            wait_for_output(&output_rx, &mut output, b"Resumed", "resumed transcript");
        } else if runner == "appearance" {
            for (command, expected) in [
                ("/theme light\r", "#f2f2f4"),
                ("/theme terminal\r", "\x1b]111"),
                ("/theme dark\r", "Dark"),
                ("/web on\r", "Web enabled"),
                ("/web off\r", "disabled"),
            ] {
                writer
                    .write_all(command.as_bytes())
                    .expect("appearance command");
                writer.flush().expect("flush appearance command");
                // Inspect fresh output: a previous frame may contain the same control label.
                let mut command_output = Vec::new();
                wait_for_output(
                    &output_rx,
                    &mut command_output,
                    expected.as_bytes(),
                    expected,
                );
                output.extend(command_output);
            }
        } else if runner == "gift" {
            writer
                .write_all(b"/redeem\r")
                .expect("open private gift entry");
            writer.flush().unwrap();
            wait_for_output(&output_rx, &mut output, b"hidden", "masked gift entry");
            writer
                .write_all(b"\x1b[200~AXG-ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567\x1b[201~")
                .unwrap();
            writer.flush().unwrap();
            wait_for_output(
                &output_rx,
                &mut output,
                b"********",
                "masked pasted gift code",
            );
            writer.write_all(b"\x1b").unwrap();
            writer.flush().unwrap();
            thread::sleep(Duration::from_millis(100));
            let completed_before = completed_turns(state_path);
            writer.write_all(b"aftergift\r").unwrap();
            writer.flush().unwrap();
            wait_for_output(&output_rx, &mut output, b"aftergift", "composer restored");
            wait_for_completed_turn(state_path, &completed_before, &output_rx, &mut output);
        } else if runner == "composer" {
            let completed_before = completed_turns(state_path);
            writer
                .write_all(b"\x1b[200~first line\nsecond line\x1b[201~\x1b[D!\x1b\rthird\r")
                .expect("paste, edit, and submit multiline prompt");
            writer.flush().expect("flush multiline prompt");
            wait_for_output(
                &output_rx,
                &mut output,
                b"thirde",
                "edited multiline prompt",
            );
            wait_for_completed_turn(state_path, &completed_before, &output_rx, &mut output);
        } else {
            thread::sleep(Duration::from_millis(250));
        }
        if matches!(
            test_runner,
            "questions" | "approval" | "plan_review" | "workspace_edit"
        ) {
            // Answer submission does not acknowledge that the runner and its
            // terminal UI event have finished. Escape must test idle focus.
            wait_for_completed_turn(state_path, &completed_before_probe, &output_rx, &mut output);
        }
    }
    if !panic_probe {
        writer.write_all(&[27]).expect("escape");
        writer.flush().expect("flush escape");
        if runner == "blocking" {
            wait_for_output(
                &output_rx,
                &mut output,
                b"Cancelled",
                "turn cancellation after Escape",
            );
        } else {
            wait_for_output(
                &output_rx,
                &mut output,
                b"\x1b[?25l",
                "hidden cursor after cleared focus",
            );
        }
        assert!(
            child.try_wait().expect("poll after escape").is_none(),
            "Escape unexpectedly exited the TUI"
        );

        writer.write_all(&[3]).expect("first Ctrl+C");
        writer.flush().expect("flush first Ctrl+C");
        // Ratatui may repaint only the changed suffix of "Ctrl+C", inserting
        // cursor-control bytes inside that visual string. "again" is the
        // stable, unique contiguous part of the confirmation.
        wait_for_output(&output_rx, &mut output, b"again", "exit confirmation");
        assert!(
            child.try_wait().expect("poll after first Ctrl+C").is_none(),
            "the first Ctrl+C unexpectedly exited the TUI"
        );
        writer.write_all(&[3]).expect("second Ctrl+C");
        writer.flush().expect("flush second Ctrl+C");
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll TUI") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().expect("kill stuck TUI");
            panic!("TUI did not exit before the PTY test deadline");
        }
        thread::sleep(Duration::from_millis(80));
    };
    if let Some(listener) = signed_out_service {
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "signed-out startup must not make service requests"
        );
    }
    drop(writer);
    reader_thread.join().expect("reader thread");
    while let Ok(chunk) = output_rx.try_recv() {
        output.extend_from_slice(&chunk);
    }
    let output = String::from_utf8_lossy(&output).into_owned();
    (output, status.success())
}

// Terminal delta redraws are not a stable text protocol. Wait for the persisted
// terminal event instead of assuming the deterministic runner finishes in 500 ms.
fn completed_turns(state_path: &Path) -> std::collections::HashSet<String> {
    let database = test_account_database(state_path);
    if !database.exists() {
        return std::collections::HashSet::new();
    }
    // SessionStore::open performs crash recovery; monitoring a live owner must
    // use a read-only connection so it cannot interrupt the running test turn.
    let connection =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read PTY session state");
    let mut statement = connection
        .prepare("SELECT id FROM turns WHERE status = 'completed'")
        .expect("query completed PTY turns");
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("read completed PTY turns")
        .collect::<rusqlite::Result<_>>()
        .expect("decode completed PTY turns")
}

fn wait_for_completed_turn(
    state_path: &Path,
    previous: &std::collections::HashSet<String>,
    receiver: &mpsc::Receiver<Vec<u8>>,
    output: &mut Vec<u8>,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while completed_turns(state_path).is_subset(previous) {
        while let Ok(chunk) = receiver.try_recv() {
            output.extend_from_slice(&chunk);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "PTY runner did not complete a new turn; output: {:?}",
            String::from_utf8_lossy(output)
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_output(
    receiver: &mpsc::Receiver<Vec<u8>>,
    output: &mut Vec<u8>,
    needle: &[u8],
    label: &str,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !output.windows(needle.len()).any(|window| window == needle) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for {label}");
        let chunk = receiver.recv_timeout(remaining).unwrap_or_else(|error| {
            panic!(
                "failed while waiting for {label}: {error}; output: {:?}",
                String::from_utf8_lossy(output)
            )
        });
        output.extend_from_slice(&chunk);
    }
}

#[test]
fn usage_command_opens_an_inspector_without_submitting_a_prompt() {
    let root = tempfile::tempdir().unwrap();
    let (output, success) = run_tui_at("usage", false, root.path(), &[]);
    assert!(success, "usage session failed: {output:?}");
    assert!(output.contains("Usage"));
    let store = SessionStore::open(test_account_database(root.path())).unwrap();
    let sessions = store.list(false).unwrap();
    assert!(!sessions.is_empty());
    for session in sessions {
        let id = session.id.parse().unwrap();
        let loaded = store.load(&id).unwrap();
        assert!(!loaded.iter().any(|envelope| matches!(&envelope.event,
            axiomcli::app::AppEvent::PromptAccepted { text, .. } if text == "/usage")));
    }
}

fn saved_prompts(root: &Path) -> Vec<String> {
    let store = SessionStore::open(test_account_database(root)).unwrap();
    store
        .list(false)
        .unwrap()
        .into_iter()
        .flat_map(|session| {
            store
                .load(&session.id.parse().unwrap())
                .unwrap()
                .into_iter()
                .filter_map(|event| match event.event {
                    axiomcli::app::AppEvent::PromptAccepted { text, .. } => Some(text),
                    _ => None,
                })
        })
        .collect()
}

#[test]
fn active_input_keeps_commands_local_and_preserves_uppercase_and_paste() {
    let root = tempfile::tempdir().unwrap();
    let (output, success) = run_tui_at("input_active", false, root.path(), &[]);
    assert!(success, "active input session failed: {output:?}");
    assert!(
        !output.contains("Steering"),
        "commands must never enter steering"
    );
    assert_eq!(
        saved_prompts(root.path()),
        ["probe", "aBcD PaStEd\nFollowUp"]
    );
}

#[test]
fn signed_out_production_runner_reaches_login_without_model_discovery() {
    let root = tempfile::tempdir().unwrap();
    let (output, success) = run_tui_at("signed_out", false, root.path(), &[]);
    assert!(success, "signed-out TUI failed: {output:?}");
    assert!(output.contains("Sign in"));
    assert!(
        std::fs::read_dir(root.path().join("data/axiom/accounts"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn model_picker_paste_filters_without_polluting_the_next_prompt() {
    let root = tempfile::tempdir().unwrap();
    let (output, success) = run_tui_at("paste_picker", false, root.path(), &[]);
    assert!(success, "picker session failed: {output:?}");
    assert_eq!(saved_prompts(root.path()), ["probe", "AfterPicker"]);
    let store = SessionStore::open(test_account_database(root.path())).unwrap();
    let session = store.list(false).unwrap().pop().unwrap();
    let events = store.load(&session.id.parse().unwrap()).unwrap();
    assert_eq!(model_for_resume(&events, "fallback"), "alternate-model");
}

#[test]
fn resize_prompt_and_double_ctrl_c_restore_the_terminal() {
    let (output, success) = run_tui("echo", false);
    assert!(success, "TUI failed; output: {output:?}");
    assert!(output.contains("Axiom"));
    assert!(
        output.contains("Full Access"),
        "slash command did not render"
    );
    assert!(
        output.contains("Thinking set to high"),
        "invalid Enter completion or slash focus shortcut did not run"
    );
    assert!(
        output.contains("Model changed") && output.contains("alternate-model"),
        "model picker did not select the keyboard-highlighted model"
    );
    assert!(
        !output.contains("\u{1b}[?1000h") && !output.contains("\u{1b}[?1006h"),
        "terminal pointer capture must remain disabled so native selection works"
    );
    assert!(output.contains("\u{1b}[?1049h"), "alternate screen entered");
    assert!(
        output.contains("\u{1b}[?1049l"),
        "alternate screen restored"
    );
}

#[test]
fn composer_preserves_multiline_paste_newline_shortcut_and_cursor_edits() {
    let (output, success) = run_tui("composer", false);
    assert!(success, "TUI failed; output: {output:?}");
    assert!(output.contains("first"), "first pasted line missing");
    assert!(
        output.contains("lin!") && output.contains("thirde"),
        "cursor insertion was not preserved: {output:?}"
    );
}

#[test]
fn escape_cancels_active_work_then_double_ctrl_c_exits_and_restores_the_terminal() {
    let (output, success) = run_tui("blocking", false);
    assert!(success, "TUI failed; output: {output:?}");
    assert!(output.contains("Cancelled"));
    assert!(output.contains("Ctrl+C") && output.contains("again"));
    assert!(output.contains("\u{1b}[?1049l"));
}

#[test]
fn structured_questions_use_a_separate_tui_answer_composer() {
    let (output, success) = run_tui("questions", false);
    assert!(success, "TUI failed; output: {output:?}");
    assert!(
        output.contains("Choose") && output.contains("cherry") && output.contains("compact"),
        "question surface was not rendered: {output:?}"
    );
    assert!(output.contains("\u{1b}[?1049l"));
}

#[test]
fn approval_uses_a_visible_tui_card_and_resumes_the_turn() {
    let (output, success) = run_tui("approval", false);
    assert!(success, "TUI failed; output: {output:?}");
    assert!(output.contains("Permission required"));
    assert!(output.contains("Allow") && output.contains("once"));
    assert!(output.contains("allow_once"));
}

#[test]
fn plan_review_is_interactive_in_the_tui_and_persists_line_comments() {
    let state = tempfile::tempdir().expect("state");
    let workspace = tempfile::tempdir().expect("workspace");
    let cwd = workspace.path().to_string_lossy().into_owned();
    let (output, success) = run_tui_at("plan_review", false, state.path(), &["--cwd", &cwd]);
    assert!(success, "TUI plan fixture failed: {output:?}");
    assert!(
        output.contains("Inspect") && output.contains("revision_requested"),
        "plan was not rendered: {output:?}"
    );
    let plans = axiomcli::planning::PlanArtifact::list(workspace.path()).expect("plans");
    assert_eq!(plans.len(), 1);
    assert_eq!(
        plans[0].state,
        axiomcli::planning::PlanState::RevisionRequested
    );
    assert_eq!(plans[0].comments[0].start_line, 4);
    assert_eq!(plans[0].comments[0].text, "Add a verification step");
}

#[test]
fn panic_unwinds_the_terminal_guard() {
    let (output, success) = run_tui("echo", true);
    assert!(!success);
    assert!(
        output.contains("requested terminal restoration probe"),
        "panic output: {output:?}"
    );
    assert!(output.contains("\u{1b}[?1049l"));
}

#[test]
fn durable_tui_session_resumes_in_the_same_workspace() {
    let state = tempfile::tempdir().expect("shared TUI state");
    let (first_output, first_success) = run_tui_at("echo", false, state.path(), &[]);
    assert!(first_success, "first TUI failed: {first_output:?}");
    let listed = test_cli_command(state.path())
        .args(["sessions", "list"])
        .output()
        .expect("list sessions");
    assert!(listed.status.success());
    let stdout = String::from_utf8(listed.stdout).expect("session list utf8");
    // Background title generation may finish before the first TUI exits. Both
    // the prompt-derived title and the fixture's generated title are valid.
    assert!(
        stdout.contains("\tprobe\t") || stdout.contains("\tGenerated conversation title\t"),
        "initial title was not persisted: {stdout}"
    );
    let id = stdout
        .lines()
        .next()
        .and_then(|line| line.split('\t').next())
        .expect("session ID");
    let (resumed_output, resumed_success) =
        run_tui_at("echo", false, state.path(), &["--resume", id]);
    assert!(resumed_success, "resumed TUI failed: {resumed_output:?}");
    assert!(
        resumed_output.contains("Resumed")
            || resumed_output.contains("no interrupted actions replayed"),
        "resume state was not rendered: {resumed_output:?}"
    );
    assert!(
        resumed_output.contains("alternate-model")
            && resumed_output.contains("high")
            && resumed_output.contains("Full Access"),
        "resume did not preserve model, thinking, and permissions: {resumed_output:?}"
    );
}

#[test]
fn resumed_tui_sessions_reconcile_retired_models_and_changed_reasoning_support() {
    let state = tempfile::tempdir().expect("shared TUI state");
    let workspace = tempfile::tempdir().expect("workspace");
    let database = test_account_database(state.path());
    let (retired_id, changed_reasoning_id) = {
        let store = SessionStore::open(&database).expect("state store");
        (
            seed_tui_session_settings(
                &store,
                workspace.path(),
                "retired-model",
                ThinkingLevel::High,
            ),
            seed_tui_session_settings(
                &store,
                workspace.path(),
                "medium-only",
                ThinkingLevel::ExtraHigh,
            ),
        )
    };
    let cwd = workspace.path().to_string_lossy().into_owned();

    for session_id in [&retired_id, &changed_reasoning_id] {
        let session_id = session_id.to_string();
        let (output, success) = run_tui_at(
            "resume_reconcile",
            false,
            state.path(),
            &["--cwd", &cwd, "--resume", &session_id],
        );
        assert!(success, "reconciled TUI resume failed: {output:?}");
    }

    let store = SessionStore::open(&database).expect("reopen corrected state");
    let retired = store
        .load_recovering(&retired_id)
        .expect("load retired-model correction");
    assert_eq!(
        model_for_resume(&retired.events, "fallback"),
        "retired-model"
    );
    assert_eq!(
        thinking_for_resume(&retired.events, ThinkingLevel::Medium),
        ThinkingLevel::High
    );
    let changed_reasoning = store
        .load_recovering(&changed_reasoning_id)
        .expect("load reasoning correction");
    assert_eq!(
        model_for_resume(&changed_reasoning.events, "fallback"),
        "medium-only"
    );
    assert_eq!(
        thinking_for_resume(&changed_reasoning.events, ThinkingLevel::ExtraHigh),
        ThinkingLevel::Medium
    );
}

#[test]
fn a_new_tui_session_starts_with_the_last_model_that_ran() {
    let state = tempfile::tempdir().expect("shared TUI state");
    let (first_output, first_success) = run_tui_at("echo", false, state.path(), &[]);
    assert!(first_success, "first TUI failed: {first_output:?}");

    let (next_output, next_success) = run_tui_at("remembered_model", false, state.path(), &[]);
    assert!(next_success, "next TUI failed: {next_output:?}");
    assert!(
        next_output.contains("alternate-model") && next_output.contains("High"),
        "new session did not start with the last used model and thinking: {next_output:?}"
    );
}

#[test]
fn a_new_tui_session_reconciles_preferences_against_the_current_catalog() {
    let state = tempfile::tempdir().expect("shared TUI state");
    let database = test_account_database(state.path());

    {
        let store = SessionStore::open(&database).expect("seed retired preference");
        store
            .set_profile_preferences("retired-model", ThinkingLevel::High)
            .expect("retired model preference");
    }
    let (output, success) = run_tui_at("preference_bootstrap", false, state.path(), &[]);
    assert!(success, "TUI failed with retired preference: {output:?}");
    {
        let store = SessionStore::open(&database).expect("read corrected preference");
        let preferences = store.profile_preferences().expect("profile preferences");
        assert_eq!(preferences.model.as_deref(), Some("alternate-model"));
        assert_eq!(preferences.thinking_level, ThinkingLevel::High);
        store
            .set_profile_preferences("medium-only", ThinkingLevel::ExtraHigh)
            .expect("changed reasoning preference");
    }

    let (output, success) = run_tui_at("preference_bootstrap", false, state.path(), &[]);
    assert!(
        success,
        "TUI failed with changed reasoning support: {output:?}"
    );
    let store = SessionStore::open(&database).expect("read reconciled reasoning preference");
    let preferences = store.profile_preferences().expect("profile preferences");
    assert_eq!(preferences.model.as_deref(), Some("medium-only"));
    assert_eq!(preferences.thinking_level, ThinkingLevel::Medium);
}

#[test]
fn slash_delete_multiselects_and_permanently_removes_saved_transcripts() {
    let state = tempfile::tempdir().expect("shared TUI state");
    for _ in 0..2 {
        let (output, success) = run_tui_at("echo", false, state.path(), &[]);
        assert!(success, "fixture TUI failed: {output:?}");
    }

    let (output, success) = run_tui_at("delete", false, state.path(), &[]);
    assert!(success, "delete picker TUI failed: {output:?}");
    assert!(output.contains("Delete transcripts"));
    assert!(output.contains("Permanently delete"));
    assert!(output.contains("Deleted"));

    let listed = test_cli_command(state.path())
        .args(["sessions", "list"])
        .output()
        .expect("list sessions after deletion");
    assert!(listed.status.success());
    assert_eq!(
        String::from_utf8(listed.stdout)
            .expect("session list utf8")
            .lines()
            .count(),
        1,
        "only the active deletion session should remain"
    );
}

#[test]
fn slash_resume_picker_searches_and_restores_a_saved_transcript() {
    let state = tempfile::tempdir().expect("shared TUI state");
    let (first_output, first_success) = run_tui_at("echo", false, state.path(), &[]);
    assert!(first_success, "first TUI failed: {first_output:?}");
    let listed = test_cli_command(state.path())
        .args(["sessions", "list"])
        .output()
        .expect("list sessions");
    let id = String::from_utf8(listed.stdout)
        .expect("session list utf8")
        .lines()
        .next()
        .and_then(|line| line.split('\t').next())
        .expect("session ID")
        .to_owned();
    let renamed = test_cli_command(state.path())
        .args(["sessions", "rename", &id, "Earlier cherry task"])
        .output()
        .expect("rename session");
    assert!(renamed.status.success());

    let (output, success) = run_tui_at("resume", false, state.path(), &[]);
    assert!(success, "resume picker TUI failed: {output:?}");
    assert!(output.contains("Resume transcript"));
    assert!(output.contains("Earlier cherry task"));
    assert!(output.contains("Resumed"));
    assert!(
        output.contains("Full") && output.contains("access"),
        "saved permission state was not restored"
    );
}

#[test]
fn workspace_edit_reaches_the_expected_tui_semantics_and_final_workspace() {
    let state = tempfile::tempdir().expect("state");
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("frontend.txt"), "before\n").expect("fixture");
    let cwd = workspace.path().to_string_lossy().into_owned();
    let (output, success) = run_tui_at("workspace_edit", false, state.path(), &["--cwd", &cwd]);
    assert!(success, "TUI fixture failed: {output:?}");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("frontend.txt")).expect("result"),
        "after\n"
    );
    assert!(
        output.contains("frontend.txt"),
        "missing edited-file feedback"
    );
    assert!(
        output.contains("Edited") || output.contains("Changed"),
        "missing folded workspace-change summary"
    );
    let listed = test_cli_command(state.path())
        .args(["sessions", "list"])
        .output()
        .expect("list TUI session");
    assert!(listed.status.success());
    let session_id = String::from_utf8(listed.stdout)
        .expect("session list")
        .lines()
        .next()
        .and_then(|line| line.split('\t').next())
        .expect("session ID")
        .to_owned();
    let exported = test_cli_command(state.path())
        .args(["sessions", "show", &session_id])
        .output()
        .expect("export TUI session");
    assert!(exported.status.success());
    let state: serde_json::Value = serde_json::from_slice(&exported.stdout).expect("state JSON");
    let timeline = state["timeline_items"].as_array().expect("timeline items");
    assert!(
        timeline.iter().any(|item| {
            item["kind"] == "tool_call"
                && item["status"] == "completed"
                && item["external_id"] == "frontend-edit"
                && item["metadata"]["name"] == "replace_text"
                && item["metadata"]["success"] == true
                && item["metadata"]["diff"]
                    .as_str()
                    .is_some_and(|diff| diff.contains("-before") && diff.contains("+after"))
                && item["metadata"]["files"].as_array().is_some_and(|files| {
                    files.iter().any(|file| {
                        file["path"]
                            .as_str()
                            .is_some_and(|path| path.ends_with("frontend.txt"))
                    })
                })
        }),
        "missing completed workspace-edit tool item: {state}"
    );
    assert!(
        timeline.iter().any(|item| {
            item["kind"] == "notice"
                && item["status"] == "completed"
                && item["metadata"]["code"] == "workspace_changed"
        }),
        "missing workspace-change notice: {state}"
    );
    assert!(
        timeline.iter().any(|item| {
            item["kind"] == "assistant_message"
                && item["status"] == "completed"
                && item["content"] == "Updated frontend.txt through the shared turn contract."
        }),
        "missing final assistant item: {state}"
    );
    assert!(output.contains("\u{1b}[?1049l"));
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "CPU/RSS benchmark; run scripts/axiomcli-eval deterministic explicitly"]
fn idle_tui_stays_within_linux_memory_and_cpu_budgets() {
    let state = tempfile::tempdir().expect("state");
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open PTY");
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_axiomcli"));
    command.arg("tui");
    command.env("AXIOMCLI_TEST_RUNNER", "echo");
    command.env("AXIOMCLI_ASCII", "1");
    command.env("NO_COLOR", "1");

    command.env("XDG_CONFIG_HOME", state.path().join("config"));
    command.env("XDG_DATA_HOME", state.path().join("data"));
    let mut child = pair.slave.spawn_command(command).expect("spawn TUI");
    drop(pair.slave);
    let pid = child.process_id().expect("TUI process ID");
    let mut reader = pair.master.try_clone_reader().expect("PTY reader");
    let reader_thread = thread::spawn(move || {
        let mut sink = std::io::sink();
        std::io::copy(&mut reader, &mut sink).expect("drain PTY");
    });
    let mut writer = pair.master.take_writer().expect("PTY writer");
    writer.write_all(b"\x1b[1;1R").expect("cursor response");
    writer.flush().expect("flush cursor response");
    thread::sleep(Duration::from_millis(300));

    let first_ticks = linux_process_ticks(pid);
    thread::sleep(Duration::from_secs(1));
    let second_ticks = linux_process_ticks(pid);
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).expect("process status");
    let rss_kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
        .expect("VmRSS");
    assert!(rss_kib < 64 * 1024, "idle RSS was {rss_kib} KiB");
    assert!(
        second_ticks.saturating_sub(first_ticks) <= 20,
        "idle TUI used more than 20 scheduler ticks in one second"
    );
    eprintln!(
        "idle TUI summary: rss_kib={rss_kib} scheduler_ticks_1s={}",
        second_ticks.saturating_sub(first_ticks)
    );

    writer.write_all(&[3]).expect("first Ctrl+C");
    writer.flush().expect("flush first Ctrl+C");
    thread::sleep(Duration::from_millis(100));
    assert!(
        child.try_wait().expect("poll after first Ctrl+C").is_none(),
        "the first Ctrl+C unexpectedly exited the idle TUI"
    );
    writer.write_all(&[3]).expect("second Ctrl+C");
    writer.flush().expect("flush second Ctrl+C");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().expect("poll TUI").is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().expect("kill stuck TUI");
            panic!("idle TUI did not exit");
        }
        thread::sleep(Duration::from_millis(50));
    }
    drop(writer);
    reader_thread.join().expect("PTY reader");
}

#[cfg(target_os = "linux")]
fn linux_process_ticks(pid: u32) -> u64 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("process stat");
    let fields = stat
        .split_once(") ")
        .expect("stat command boundary")
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let user = fields[11].parse::<u64>().expect("user ticks");
    let system = fields[12].parse::<u64>().expect("system ticks");
    user + system
}

#[test]
fn appearance_and_web_controls_work_in_a_real_terminal_and_restore_background() {
    let (output, success) = run_tui("appearance", false);
    assert!(success, "appearance session failed: {output:?}");
    assert!(output.contains("#f2f2f4"));
    assert!(output.contains("#141416"));
    assert!(output.contains("Web enabled"));
    assert!(output.contains("\x1b]111"));
    assert!(output.contains("\x1b[?1049l"));
}

#[test]
fn gift_entry_never_echoes_or_persists_the_code_as_chat() {
    let directory = tempfile::tempdir().unwrap();
    let (output, success) = run_tui_at("gift", false, directory.path(), &[]);
    assert!(success);
    assert!(!output.contains("AXG-ABCD"));
    let data = std::fs::read(test_account_database(directory.path())).unwrap();
    assert!(
        !data
            .windows(b"AXG-ABCD".len())
            .any(|bytes| bytes == b"AXG-ABCD")
    );
}
