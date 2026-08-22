#![cfg(unix)]

use std::{
    io::{Read as _, Write as _},
    path::Path,
    sync::mpsc,
    thread,
    time::Duration,
};

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
    if runner == "workspace_edit" {
        command.env("AXIOM_PERMISSION_PROFILE", "workspace");
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
        // "view" is clipped to "vi" at 60 columns and appears in full only
        // after the 100-column resize, proving the input loop is active.
        wait_for_output(
            &output_rx,
            &mut output,
            b"view",
            "resized interactive screen",
        );
        writer.write_all(b"probe\r").expect("submit prompt");
        writer.flush().expect("flush prompt");
        if runner == "blocking" {
            wait_for_output(&output_rx, &mut output, b"Working", "blocking turn");
            writer.write_all(&[3]).expect("Ctrl+C");
            writer.flush().expect("flush Ctrl+C");
            thread::sleep(Duration::from_millis(180));
        } else if runner == "questions" {
            wait_for_output(&output_rx, &mut output, b"1=cherry", "first question");
            writer.write_all(b"1\r").expect("first answer");
            writer.flush().expect("flush first answer");
            writer.write_all(b"compact\r").expect("second answer");
            writer.flush().expect("flush second answer");
            thread::sleep(Duration::from_millis(180));
        } else if runner == "plan_review" {
            wait_for_output(&output_rx, &mut output, b"1=approve", "review decision");
            writer.write_all(b"2\r").expect("revision decision");
            writer.flush().expect("flush revision decision");
            writer.write_all(b"4\r").expect("revision line");
            writer.flush().expect("flush revision line");
            writer
                .write_all(b"Add a verification step\r")
                .expect("revision comment");
            writer.flush().expect("flush revision comment");
            thread::sleep(Duration::from_millis(180));
        } else {
            thread::sleep(Duration::from_millis(250));
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
    reader_thread.join().expect("reader thread");
    while let Ok(chunk) = output_rx.try_recv() {
        output.extend_from_slice(&chunk);
    }
    let output = String::from_utf8_lossy(&output).into_owned();
    (output, status.success())
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
fn plan_review_is_interactive_in_the_tui_and_persists_line_comments() {
    let state = tempfile::tempdir().expect("state");
    let workspace = tempfile::tempdir().expect("workspace");
    let cwd = workspace.path().to_string_lossy().into_owned();
    let (output, success) = run_tui_at("plan_review", false, state.path(), &["--cwd", &cwd]);
    assert!(success, "TUI plan fixture failed: {output:?}");
    assert!(
        output.contains("PLAN r1"),
        "plan was not rendered: {output:?}"
    );
    assert!(
        output.contains("revision_requested"),
        "review result was not rendered: {output:?}"
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
    assert!(output.contains("replace_text"), "missing tool identity");
    assert!(output.contains("frontend.txt"), "missing workspace path");
    let listed = std::process::Command::new(env!("CARGO_BIN_EXE_axiomcli"))
        .args(["sessions", "list"])
        .env("XDG_CONFIG_HOME", state.path().join("config"))
        .env("XDG_DATA_HOME", state.path().join("data"))
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
    let exported = std::process::Command::new(env!("CARGO_BIN_EXE_axiomcli"))
        .args(["sessions", "show", &session_id])
        .env("XDG_CONFIG_HOME", state.path().join("config"))
        .env("XDG_DATA_HOME", state.path().join("data"))
        .output()
        .expect("export TUI session");
    assert!(exported.status.success());
    let journal = String::from_utf8(exported.stdout).expect("journal JSON");
    assert!(
        journal.contains("Updated frontend.txt through the shared turn contract.")
            && journal.contains("diff_available")
            && journal.contains("tool_completed"),
        "missing shared semantic events: {journal}"
    );
    assert!(output.contains("\u{1b}[?1049l"));
}

#[test]
#[cfg(target_os = "linux")]
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

    writer.write_all(&[27]).expect("quit TUI");
    writer.flush().expect("flush quit");
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
