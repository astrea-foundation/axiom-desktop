use std::path::Path;

use serde_json::Value;

use crate::audit::redact_text;

const MAX_DETAIL_CHARS: usize = 140;
const MAX_LABEL_CHARS: usize = 220;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolDisplayPhase {
    Proposed,
    Running,
    Completed,
    Failed,
}

/// Turn a provider-neutral tool call into a concise, safe activity label.
///
/// Raw arguments remain available in the tool details. This formatter is only
/// for the high-signal line users see while work is happening, so it bounds
/// untrusted text, removes controls, redacts recognizable credentials, and
/// presents paths relative to the workspace whenever that is unambiguous.
pub(crate) fn describe_tool(
    name: &str,
    arguments: &Value,
    phase: ToolDisplayPhase,
    cwd: Option<&Path>,
) -> String {
    let label = match name {
        "read_file" => describe_path_action(arguments, "path", phase, cwd, PathAction::Read),
        "list_files" => describe_path_action(arguments, "path", phase, cwd, PathAction::List),
        "inspect_metadata" => {
            describe_path_action(arguments, "path", phase, cwd, PathAction::Inspect)
        }
        "replace_text" => describe_path_action(arguments, "path", phase, cwd, PathAction::Edit),
        "glob_files" => describe_query_action(
            argument_text(arguments, "pattern").as_deref(),
            phase,
            QueryAction::Glob,
        ),
        "search_text" => describe_query_action(
            argument_text(arguments, "query").as_deref(),
            phase,
            QueryAction::Workspace,
        ),
        "select_context" => describe_query_action(
            argument_text(arguments, "query").as_deref(),
            phase,
            QueryAction::Context,
        ),
        "web_search" => describe_query_action(
            argument_text(arguments, "query").as_deref(),
            phase,
            QueryAction::Web,
        ),
        "fetch_url" => describe_query_action(
            argument_text(arguments, "url").as_deref(),
            phase,
            QueryAction::Fetch,
        ),
        "inspect_git" => phased(
            phase,
            "Inspecting Git status and changes",
            "Inspected Git status and changes",
            "Failed to inspect Git status and changes",
        )
        .into(),
        "view_session_diff" => phased(
            phase,
            "Reviewing this session's changes",
            "Reviewed this session's changes",
            "Failed to review this session's changes",
        )
        .into(),
        "apply_patch" => describe_patch(arguments, phase, cwd),
        "run_command" => describe_command(arguments, phase, cwd, CommandKind::Foreground),
        "run_shell" => describe_command(arguments, phase, cwd, CommandKind::Shell),
        "start_background" => describe_command(arguments, phase, cwd, CommandKind::Background),
        "background_list" => phased(
            phase,
            "Checking background tasks",
            "Checked background tasks",
            "Failed to check background tasks",
        )
        .into(),
        "background_status" => describe_task_action(arguments, phase, TaskAction::Inspect),
        "background_wait" => describe_task_action(arguments, phase, TaskAction::Wait),
        "stop_background" => describe_task_action(arguments, phase, TaskAction::Stop),
        "plan_create" => phased(
            phase,
            "Creating a plan",
            "Created a plan",
            "Failed to create a plan",
        )
        .into(),
        "plan_revise" => describe_plan_action(arguments, phase, PlanAction::Revise),
        "plan_propose" => describe_plan_action(arguments, phase, PlanAction::Propose),
        "ask_user_questions" => describe_questions(arguments, phase),
        "update_progress" => describe_progress(arguments, phase),
        _ if name.starts_with("mcp__") => describe_mcp(name, arguments, phase, cwd),
        _ => describe_generic(name, arguments, phase, cwd),
    };
    truncate_chars(&redact_text(&label), MAX_LABEL_CHARS)
}

#[derive(Clone, Copy)]
enum PathAction {
    Read,
    List,
    Inspect,
    Edit,
}

fn describe_path_action(
    arguments: &Value,
    key: &str,
    phase: ToolDisplayPhase,
    cwd: Option<&Path>,
    action: PathAction,
) -> String {
    let (active, completed, failed) = match action {
        PathAction::Read => ("Reading", "Read", "Failed to read"),
        PathAction::List => ("Listing", "Listed", "Failed to list"),
        PathAction::Inspect => ("Inspecting", "Inspected", "Failed to inspect"),
        PathAction::Edit => ("Editing", "Edited", "Failed to edit"),
    };
    let verb = phased(phase, active, completed, failed);
    argument_path(arguments, key, cwd).map_or_else(
        || {
            let fallback = match action {
                PathAction::Inspect => "file metadata",
                PathAction::Read | PathAction::List | PathAction::Edit => "files",
            };
            format!("{verb} {fallback}")
        },
        |target| format!("{verb} {}", inline(&target)),
    )
}

#[derive(Clone, Copy)]
enum QueryAction {
    Glob,
    Workspace,
    Context,
    Web,
    Fetch,
}

fn describe_query_action(
    target: Option<&str>,
    phase: ToolDisplayPhase,
    action: QueryAction,
) -> String {
    let target = target.map(inline);
    match action {
        QueryAction::Glob => target.map_or_else(
            || {
                phased(
                    phase,
                    "Finding files",
                    "Found matching files",
                    "Failed to find files",
                )
                .into()
            },
            |target| {
                format!(
                    "{} {target}",
                    phased(
                        phase,
                        "Finding files matching",
                        "Found files matching",
                        "Failed to find files matching"
                    )
                )
            },
        ),
        QueryAction::Workspace => target.map_or_else(
            || {
                phased(
                    phase,
                    "Searching files",
                    "Searched files",
                    "Failed to search files",
                )
                .into()
            },
            |target| {
                format!(
                    "{} {target}",
                    phased(
                        phase,
                        "Searching files for",
                        "Searched files for",
                        "Failed to search files for"
                    )
                )
            },
        ),
        QueryAction::Context => target.map_or_else(
            || {
                phased(
                    phase,
                    "Selecting relevant context",
                    "Selected relevant context",
                    "Failed to select relevant context",
                )
                .into()
            },
            |target| {
                format!(
                    "{} {target}",
                    phased(
                        phase,
                        "Selecting context for",
                        "Selected context for",
                        "Failed to select context for"
                    )
                )
            },
        ),
        QueryAction::Web => target.map_or_else(
            || {
                phased(
                    phase,
                    "Searching the web",
                    "Searched the web",
                    "Failed to search the web",
                )
                .into()
            },
            |target| {
                format!(
                    "{} {target}",
                    phased(
                        phase,
                        "Searching the web for",
                        "Searched the web for",
                        "Failed to search the web for"
                    )
                )
            },
        ),
        QueryAction::Fetch => target.map_or_else(
            || {
                phased(
                    phase,
                    "Fetching a page",
                    "Fetched a page",
                    "Failed to fetch a page",
                )
                .into()
            },
            |target| {
                format!(
                    "{} {target}",
                    phased(phase, "Fetching", "Fetched", "Failed to fetch")
                )
            },
        ),
    }
}

fn describe_patch(arguments: &Value, phase: ToolDisplayPhase, cwd: Option<&Path>) -> String {
    let paths = arguments
        .get("edits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edit| edit.get("path").and_then(Value::as_str))
        .map(|path| pretty_path(path, cwd))
        .collect::<Vec<_>>();
    let action = phased(phase, "Editing", "Edited", "Failed to edit");
    match paths.as_slice() {
        [] => format!("{action} files"),
        [path] => format!("{action} {}", inline(path)),
        paths => {
            let shown = paths
                .iter()
                .take(2)
                .map(|path| inline(path))
                .collect::<Vec<_>>()
                .join(", ");
            let remainder = paths.len().saturating_sub(2);
            if remainder == 0 {
                format!("{action} {} files: {shown}", paths.len())
            } else {
                format!("{action} {} files: {shown}, +{remainder} more", paths.len())
            }
        }
    }
}

#[derive(Clone, Copy)]
enum CommandKind {
    Foreground,
    Shell,
    Background,
}

fn describe_command(
    arguments: &Value,
    phase: ToolDisplayPhase,
    cwd: Option<&Path>,
    kind: CommandKind,
) -> String {
    let (command, git) = match kind {
        CommandKind::Shell => {
            let command = argument_text(arguments, "command").unwrap_or_else(|| "shell".into());
            let git = shell_starts_with_git(&command);
            (redact_shell_command(&command), git)
        }
        CommandKind::Foreground | CommandKind::Background => {
            let program = argument_text(arguments, "program").unwrap_or_else(|| "command".into());
            let git = Path::new(&program)
                .file_name()
                .is_some_and(|name| name == "git");
            (structured_command(&program, arguments.get("args")), git)
        }
    };
    let action = match (kind, git, phase) {
        (CommandKind::Background, _, ToolDisplayPhase::Completed) => "Started background command",
        (CommandKind::Background, _, ToolDisplayPhase::Failed) => {
            "Failed to start background command"
        }
        (CommandKind::Background, _, _) => "Starting background command",
        (_, true, ToolDisplayPhase::Completed) => "Ran Git command",
        (_, true, ToolDisplayPhase::Failed) => "Git command failed",
        (_, true, _) => "Running Git command",
        (_, false, ToolDisplayPhase::Completed) => "Ran a command",
        (_, false, ToolDisplayPhase::Failed) => "Command failed",
        (_, false, _) => "Running a command",
    };
    let location = argument_text(arguments, "cwd")
        .filter(|path| !path.is_empty() && path != ".")
        .map(|path| format!(" in {}", inline(&pretty_path(&path, cwd))))
        .unwrap_or_default();
    format!("{action}: {}{location}", inline(&command))
}

#[derive(Clone, Copy)]
enum TaskAction {
    Inspect,
    Wait,
    Stop,
}

fn describe_task_action(arguments: &Value, phase: ToolDisplayPhase, action: TaskAction) -> String {
    let task = argument_text(arguments, "task_id").map(|id| inline(&id));
    let (active, completed, failed) = match action {
        TaskAction::Inspect => (
            "Checking background task",
            "Checked background task",
            "Failed to check background task",
        ),
        TaskAction::Wait => (
            "Waiting for background task",
            "Background task finished",
            "Background task failed",
        ),
        TaskAction::Stop => (
            "Stopping background task",
            "Stopped background task",
            "Failed to stop background task",
        ),
    };
    task.map_or_else(
        || phased(phase, active, completed, failed).into(),
        |task| format!("{} {task}", phased(phase, active, completed, failed)),
    )
}

#[derive(Clone, Copy)]
enum PlanAction {
    Revise,
    Propose,
}

fn describe_plan_action(arguments: &Value, phase: ToolDisplayPhase, action: PlanAction) -> String {
    let plan = argument_text(arguments, "plan_id").map(|id| inline(&id));
    let (active, completed, failed) = match action {
        PlanAction::Revise => ("Revising plan", "Revised plan", "Failed to revise plan"),
        PlanAction::Propose => (
            "Preparing plan for review",
            "Submitted plan for review",
            "Failed to submit plan for review",
        ),
    };
    plan.map_or_else(
        || phased(phase, active, completed, failed).into(),
        |plan| format!("{} {plan}", phased(phase, active, completed, failed)),
    )
}

fn describe_questions(arguments: &Value, phase: ToolDisplayPhase) -> String {
    let count = arguments
        .get("questions")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let noun = if count == 1 { "question" } else { "questions" };
    let count = (count > 0).then(|| format!("{count} {noun}"));
    let (active, completed, failed) = ("Preparing", "Prepared", "Failed to prepare");
    count.map_or_else(
        || format!("{} questions", phased(phase, active, completed, failed)),
        |count| format!("{} {count}", phased(phase, active, completed, failed)),
    )
}

fn describe_progress(arguments: &Value, phase: ToolDisplayPhase) -> String {
    let message = argument_text(arguments, "message").map(|message| inline(&message));
    message.map_or_else(
        || {
            phased(
                phase,
                "Updating task progress",
                "Updated task progress",
                "Failed to update task progress",
            )
            .into()
        },
        |message| {
            format!(
                "{}: {message}",
                phased(
                    phase,
                    "Updating task progress",
                    "Updated task progress",
                    "Failed to update task progress"
                )
            )
        },
    )
}

fn describe_mcp(
    name: &str,
    arguments: &Value,
    phase: ToolDisplayPhase,
    cwd: Option<&Path>,
) -> String {
    let mut parts = name.trim_start_matches("mcp__").splitn(2, "__");
    let server = humanize_name(parts.next().unwrap_or("MCP"));
    let tool = humanize_name(parts.next().unwrap_or("tool"));
    let action = phased(phase, "Calling", "Called", "MCP call failed for");
    let target = generic_target(arguments, cwd)
        .map(|target| format!(" · {}", inline(&target)))
        .unwrap_or_default();
    format!("{action} MCP {server}: {tool}{target}")
}

fn describe_generic(
    name: &str,
    arguments: &Value,
    phase: ToolDisplayPhase,
    cwd: Option<&Path>,
) -> String {
    let action = phased(phase, "Using", "Used", "Failed to use");
    let target = generic_target(arguments, cwd)
        .map(|target| format!(" · {}", inline(&target)))
        .unwrap_or_default();
    format!("{action} {}{target}", humanize_name(name))
}

fn generic_target(arguments: &Value, cwd: Option<&Path>) -> Option<String> {
    for key in ["path", "file", "file_path"] {
        if let Some(path) = argument_text(arguments, key) {
            return Some(pretty_path(&path, cwd));
        }
    }
    for key in [
        "query",
        "pattern",
        "url",
        "command",
        "repository",
        "repo",
        "task_id",
        "id",
    ] {
        if let Some(value) = argument_text(arguments, key) {
            return Some(value);
        }
    }
    None
}

fn argument_path(arguments: &Value, key: &str, cwd: Option<&Path>) -> Option<String> {
    argument_text(arguments, key).map(|path| pretty_path(&path, cwd))
}

fn argument_text(arguments: &Value, key: &str) -> Option<String> {
    let value = arguments.as_object()?.get(key)?.as_str()?;
    let value = compact_text(value);
    (!value.is_empty()).then(|| truncate_chars(&value, MAX_DETAIL_CHARS))
}

fn pretty_path(raw: &str, cwd: Option<&Path>) -> String {
    let path = Path::new(raw);
    let shown = if path.is_absolute() {
        cwd.and_then(|cwd| path.strip_prefix(cwd).ok())
            .map_or(path, |relative| {
                if relative.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    relative
                }
            })
    } else {
        path.strip_prefix(".").unwrap_or(path)
    };
    let shown = shown.to_string_lossy();
    let shown = if shown.is_empty() { "." } else { &shown };
    truncate_chars(&compact_text(shown), MAX_DETAIL_CHARS)
}

fn structured_command(program: &str, arguments: Option<&Value>) -> String {
    let mut parts = vec![shell_quote(&redact_command_argument(program, false))];
    let mut redact_next = false;
    if let Some(arguments) = arguments.and_then(Value::as_array) {
        for argument in arguments.iter().filter_map(Value::as_str) {
            let redacted = redact_command_argument(argument, redact_next);
            redact_next = is_sensitive_flag(argument) && !argument.contains('=');
            parts.push(shell_quote(&redacted));
        }
    }
    truncate_chars(&redact_text(&parts.join(" ")), MAX_DETAIL_CHARS)
}

fn redact_shell_command(command: &str) -> String {
    let mut redact_next = false;
    let mut parts = Vec::new();
    for part in command.split_whitespace() {
        parts.push(redact_command_argument(part, redact_next));
        redact_next = is_sensitive_flag(part) && !part.contains('=');
    }
    truncate_chars(&redact_text(&parts.join(" ")), MAX_DETAIL_CHARS)
}

fn redact_command_argument(argument: &str, force: bool) -> String {
    if force {
        return "[REDACTED]".into();
    }
    if let Some((flag, _)) = argument.split_once('=')
        && is_sensitive_flag(flag)
    {
        return format!("{flag}=[REDACTED]");
    }
    redact_text(argument)
}

fn is_sensitive_flag(argument: &str) -> bool {
    let flag = argument
        .split_once('=')
        .map_or(argument, |(flag, _)| flag)
        .trim_start_matches('-')
        .to_ascii_lowercase()
        .replace('-', "_");
    [
        "api_key",
        "apikey",
        "authorization",
        "cookie",
        "password",
        "private_key",
        "secret",
        "token",
    ]
    .iter()
    .any(|sensitive| flag == *sensitive || flag.ends_with(&format!("_{sensitive}")))
}

fn shell_quote(argument: &str) -> String {
    if !argument.is_empty()
        && argument
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_@%+=:,./-".contains(character))
    {
        argument.into()
    } else {
        format!("'{}'", argument.replace('\'', "'\"'\"'"))
    }
}

fn shell_starts_with_git(command: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .and_then(|program| Path::new(program).file_name())
        .is_some_and(|program| program == "git")
}

fn inline(value: &str) -> String {
    format!(
        "`{}`",
        truncate_chars(&compact_text(value), MAX_DETAIL_CHARS).replace('`', "′")
    )
}

fn compact_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut characters = value.chars();
    let mut bounded = characters.by_ref().take(max_chars).collect::<String>();
    if characters.next().is_some() {
        bounded.push('…');
    }
    bounded
}

fn humanize_name(name: &str) -> String {
    name.replace(['_', '-'], " ")
}

const fn phased(
    phase: ToolDisplayPhase,
    active: &'static str,
    completed: &'static str,
    failed: &'static str,
) -> &'static str {
    match phase {
        ToolDisplayPhase::Proposed | ToolDisplayPhase::Running => active,
        ToolDisplayPhase::Completed => completed,
        ToolDisplayPhase::Failed => failed,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;

    #[test]
    fn paths_are_relative_inside_the_workspace_and_absolute_outside_it() {
        let cwd = std::env::current_dir().expect("absolute workspace");
        let outside = std::env::temp_dir().join("axiom-outside.txt");
        assert_eq!(
            describe_tool(
                "read_file",
                &json!({"path": cwd.join("src/main.rs")}),
                ToolDisplayPhase::Completed,
                Some(&cwd),
            ),
            "Read `src/main.rs`"
        );
        assert_eq!(
            describe_tool(
                "read_file",
                &json!({"path": outside}),
                ToolDisplayPhase::Completed,
                Some(&cwd),
            ),
            format!("Read `{}`", outside.display())
        );
    }

    #[test]
    fn commands_git_searches_and_urls_expose_bounded_useful_details() {
        let cwd = Path::new("/work/axiomai");
        assert_eq!(
            describe_tool(
                "run_command",
                &json!({"program":"git", "args":["status", "--short"]}),
                ToolDisplayPhase::Running,
                Some(cwd),
            ),
            "Running Git command: `git status --short`"
        );
        assert_eq!(
            describe_tool(
                "web_search",
                &json!({"query":"Rust terminal markdown rendering"}),
                ToolDisplayPhase::Completed,
                Some(cwd),
            ),
            "Searched the web for `Rust terminal markdown rendering`"
        );
        assert_eq!(
            describe_tool(
                "fetch_url",
                &json!({"url":"https://example.com/docs"}),
                ToolDisplayPhase::Running,
                Some(cwd),
            ),
            "Fetching `https://example.com/docs`"
        );
    }

    #[test]
    fn command_details_redact_secrets_and_patch_summaries_bound_file_lists() {
        let command = describe_tool(
            "run_command",
            &json!({
                "program":"client",
                "args":["--api-key", "super-secret-value", "axm_123456789abcdefgh"]
            }),
            ToolDisplayPhase::Completed,
            None,
        );
        assert_eq!(
            command,
            "Ran a command: `client --api-key '[REDACTED]' '[REDACTED]'`"
        );
        let patch = describe_tool(
            "apply_patch",
            &json!({"edits":[
                {"path":"one.rs"}, {"path":"two.rs"}, {"path":"three.rs"}
            ]}),
            ToolDisplayPhase::Completed,
            None,
        );
        assert_eq!(patch, "Edited 3 files: `one.rs`, `two.rs`, +1 more");
    }

    #[test]
    fn mcp_calls_keep_server_tool_and_a_useful_argument() {
        assert_eq!(
            describe_tool(
                "mcp__github__create_issue",
                &json!({"repo":"astrea/axiomai", "title":"bug"}),
                ToolDisplayPhase::Running,
                None,
            ),
            "Calling MCP github: create issue · `astrea/axiomai`"
        );
    }
}
