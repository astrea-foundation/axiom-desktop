#![cfg(unix)]

use std::{io::Write as _, path::Path, sync::mpsc, thread, time::Duration};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

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
    command.env("AXIOMCLI_TEST_RUNNER", runner);
    command.env("AXIOMCLI_ASCII", "1");
    command.env("NO_COLOR", "1");
    command.env("XDG_CONFIG_HOME", state_path.join("config"));
    command.env("XDG_DATA_HOME", state_path.join("data"));
    if panic_probe {
        command.env("AXIOMCLI_TEST_TUI_PANIC", "1");
    }
    let mut child = pair.slave.spawn_command(command).expect("spawn TUI in PTY");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("PTY reader");
    let (output_tx, output_rx) = mpsc::channel();
    let reader_thread = thread::spawn(move || {
        let mut output = Vec::new();
        std::io::copy(&mut reader, &mut output).expect("read PTY");
        let _ = output_tx.send(String::from_utf8_lossy(&output).into_owned());
    });

    let mut writer = pair.master.take_writer().expect("PTY writer");
    // Crossterm asks the terminal for its cursor position while initializing.
    // A real terminal answers this DSR request; the PTY harness must emulate it.
    writer
        .write_all(b"\x1b[1;1R")
        .expect("cursor position response");
    writer.flush().expect("flush cursor response");
    thread::sleep(Duration::from_millis(120));
    pair.master
        .resize(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("resize PTY");
    if !panic_probe {
        writer.write_all(b"probe\r").expect("submit prompt");
        writer.flush().expect("flush prompt");
        thread::sleep(Duration::from_millis(250));
        if runner == "blocking" {
            writer.write_all(&[3]).expect("Ctrl+C");
            writer.flush().expect("flush Ctrl+C");
            thread::sleep(Duration::from_millis(180));
        } else if runner == "questions" {
            writer.write_all(b"1\r").expect("first answer");
            writer.flush().expect("flush first answer");
            thread::sleep(Duration::from_millis(120));
            writer.write_all(b"compact\r").expect("second answer");
            writer.flush().expect("flush second answer");
            thread::sleep(Duration::from_millis(180));
        }
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
        if !panic_probe {
            writer.write_all(&[27]).expect("escape");
            writer.flush().expect("flush escape");
        }
        thread::sleep(Duration::from_millis(80));
    };
    drop(writer);
    let output = output_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("PTY reader completion");
    reader_thread.join().expect("reader thread");
    (output, status.success())
}

#[test]
fn resize_prompt_and_quit_restore_the_terminal() {
    let (output, success) = run_tui("echo", false);
    assert!(success, "TUI failed; output: {output:?}");
    assert!(output.contains("AXIOMCLI"));
    assert!(output.contains("\u{1b}[?1049h"), "alternate screen entered");
    assert!(
        output.contains("\u{1b}[?1049l"),
        "alternate screen restored"
    );
}

#[test]
fn cancellation_restores_the_terminal() {
    let (output, success) = run_tui("blocking", false);
    assert!(success, "TUI failed; output: {output:?}");
    assert!(output.contains("Cancelling") || output.contains("Cancelled"));
    assert!(output.contains("\u{1b}[?1049l"));
}

#[test]
fn structured_questions_use_a_separate_tui_answer_composer() {
    let (output, success) = run_tui("questions", false);
    assert!(success, "TUI failed; output: {output:?}");
    assert!(
        output.contains("1=cherry") && output.contains("\"theme\":[\"cherry\"]"),
        "question surface was not rendered: {output:?}"
    );
    assert!(output.contains("\u{1b}[?1049l"));
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
    let listed = std::process::Command::new(env!("CARGO_BIN_EXE_axiomcli"))
        .args(["sessions", "list"])
        .env("XDG_CONFIG_HOME", state.path().join("config"))
        .env("XDG_DATA_HOME", state.path().join("data"))
        .output()
        .expect("list sessions");
    assert!(listed.status.success());
    let stdout = String::from_utf8(listed.stdout).expect("session list utf8");
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
}
