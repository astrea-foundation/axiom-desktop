use std::{
    cell::RefCell,
    fmt::Write as _,
    path::PathBuf,
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::app::{TaskItem, TurnId};

use super::markdown::StreamingMarkdownRenderer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TaskPhase {
    Starting,
    Running,
    Waiting,
    Completed,
    Failed,
    Cancelled,
}

impl TaskPhase {
    pub(super) const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TaskActivity {
    Starting,
    Thinking,
    Responding,
    PreparingTool(String),
    RunningTool(String),
    Working(String),
    Waiting(String),
}

impl TaskActivity {
    pub(super) fn label(&self) -> String {
        match self {
            Self::Starting => "Starting the task".into(),
            Self::Thinking => "Thinking through the next step".into(),
            Self::Responding => "Writing the response".into(),
            Self::PreparingTool(name) => format!("Preparing {}", active_tool_label(name)),
            Self::RunningTool(name) => active_tool_label(name),
            Self::Working(message) | Self::Waiting(message) => message.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolPhase {
    Proposed,
    Running,
    Completed,
    Failed,
}

impl ToolPhase {
    pub(super) const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

#[derive(Clone, Debug)]
pub(super) struct ToolView {
    pub(super) call_id: String,
    pub(super) name: String,
    pub(super) arguments: Value,
    pub(super) phase: ToolPhase,
    pub(super) output: String,
    pub(super) output_truncated: bool,
    pub(super) diff: String,
    pub(super) diff_truncated: bool,
}

impl ToolView {
    pub(super) fn new(call_id: String, name: String, arguments: Value) -> Self {
        Self {
            call_id,
            name,
            arguments,
            phase: ToolPhase::Proposed,
            output: String::new(),
            output_truncated: false,
            diff: String::new(),
            diff_truncated: false,
        }
    }

    pub(super) fn completed_label(&self) -> String {
        completed_tool_label(&self.name)
    }
}

#[derive(Clone, Debug)]
pub(super) struct TaskRunView {
    pub(super) turn_id: TurnId,
    pub(super) prompt: String,
    pub(super) phase: TaskPhase,
    pub(super) activity: TaskActivity,
    pub(super) reasoning: String,
    pub(super) response: String,
    pub(super) tools: Vec<ToolView>,
    pub(super) tasks: Vec<TaskItem>,
    pub(super) changed_paths: Vec<PathBuf>,
    pub(super) error: Option<String>,
    pub(super) error_detail: Option<String>,
    pub(super) started_at: Instant,
    pub(super) finished_at: Option<Instant>,
    pub(super) response_markdown: RefCell<StreamingMarkdownRenderer>,
}

impl TaskRunView {
    pub(super) fn new(turn_id: TurnId, prompt: String) -> Self {
        Self {
            turn_id,
            prompt,
            phase: TaskPhase::Starting,
            activity: TaskActivity::Starting,
            reasoning: String::new(),
            response: String::new(),
            tools: Vec::new(),
            tasks: Vec::new(),
            changed_paths: Vec::new(),
            error: None,
            error_detail: None,
            started_at: Instant::now(),
            finished_at: None,
            response_markdown: RefCell::default(),
        }
    }

    pub(super) fn finish(&mut self, phase: TaskPhase, error: Option<String>) {
        self.phase = phase;
        self.error = error;
        self.error_detail = None;
        self.finished_at = Some(Instant::now());
    }

    pub(super) fn fail(&mut self, message: String, detail: Option<String>) {
        self.phase = TaskPhase::Failed;
        self.error = Some(message);
        self.error_detail = detail;
        self.finished_at = Some(Instant::now());
    }

    pub(super) fn elapsed(&self) -> Duration {
        self.finished_at
            .unwrap_or_else(Instant::now)
            .saturating_duration_since(self.started_at)
    }

    pub(super) fn tool_mut(&mut self, call_id: &str) -> Option<&mut ToolView> {
        self.tools.iter_mut().find(|tool| tool.call_id == call_id)
    }

    pub(super) fn ensure_tool(
        &mut self,
        call_id: &str,
        name: &str,
        arguments: Value,
    ) -> &mut ToolView {
        if let Some(index) = self.tools.iter().position(|tool| tool.call_id == call_id) {
            let tool = &mut self.tools[index];
            if tool.name == "tool" && name != "tool" {
                name.clone_into(&mut tool.name);
            }
            return tool;
        }
        self.tools.push(ToolView::new(
            call_id.to_owned(),
            name.to_owned(),
            arguments,
        ));
        self.tools.last_mut().expect("tool was just inserted")
    }

    pub(super) fn running_tool(&self) -> Option<&ToolView> {
        self.tools
            .iter()
            .rev()
            .find(|tool| !tool.phase.is_terminal())
    }

    pub(super) fn recent_completed_tools(&self, limit: usize) -> Vec<&ToolView> {
        let completed = self
            .tools
            .iter()
            .filter(|tool| tool.phase.is_terminal())
            .collect::<Vec<_>>();
        completed
            .into_iter()
            .rev()
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    pub(super) fn completion_summary(&self) -> String {
        let mut read = 0_usize;
        let mut searched = 0_usize;
        let mut ran = 0_usize;
        let mut changed = 0_usize;
        let mut used = 0_usize;
        for tool in self
            .tools
            .iter()
            .filter(|tool| tool.phase == ToolPhase::Completed)
        {
            match classify_tool(&tool.name) {
                ToolClass::Read => read += 1,
                ToolClass::Search => searched += 1,
                ToolClass::Run => ran += 1,
                ToolClass::Change => changed += 1,
                ToolClass::Other => used += 1,
            }
        }
        if !self.changed_paths.is_empty() {
            changed = changed.max(self.changed_paths.len());
        }

        let mut parts = Vec::new();
        push_count(&mut parts, read, "Read", "item", "items");
        push_count(&mut parts, searched, "Searched", "source", "sources");
        push_count(&mut parts, ran, "Ran", "command", "commands");
        push_count(&mut parts, changed, "Changed", "file", "files");
        push_count(&mut parts, used, "Used", "tool", "tools");
        let failed = self
            .tools
            .iter()
            .filter(|tool| tool.phase == ToolPhase::Failed)
            .count();
        push_count(&mut parts, failed, "Failed", "tool", "tools");
        if parts.is_empty() {
            "Finished the request".into()
        } else {
            parts.join(" · ")
        }
    }

    pub(super) fn searchable_text(&self) -> String {
        let mut text = format!("{}\n{}\n{}", self.prompt, self.reasoning, self.response);
        for task in &self.tasks {
            let _ = write!(text, "\n{}\n{}", task.title, task.id);
        }
        for tool in &self.tools {
            let _ = write!(
                text,
                "\n{}\n{}\n{}\n{}",
                tool.name, tool.call_id, tool.output, tool.diff
            );
        }
        for path in &self.changed_paths {
            let _ = write!(text, "\n{}", path.display());
        }
        if let Some(error) = &self.error {
            let _ = write!(text, "\n{error}");
        }
        if let Some(detail) = &self.error_detail {
            let _ = write!(text, "\n{detail}");
        }
        text
    }

    pub(super) fn detail_text(&self) -> String {
        let mut detail = format!("TASK\n{}\n\nTURN\n{}", self.prompt, self.turn_id);
        if !self.tasks.is_empty() {
            detail.push_str("\n\nPLAN");
            for task in &self.tasks {
                let _ = write!(detail, "\n{:?}  {}", task.status, task.title);
            }
        }
        if !self.reasoning.trim().is_empty() {
            let _ = write!(detail, "\n\nTHOUGHT\n{}", self.reasoning.trim());
        }
        if !self.tools.is_empty() {
            detail.push_str("\n\nTOOL CALLS");
            for tool in &self.tools {
                let _ = write!(
                    detail,
                    "\n\n{} · {} · {:?}\narguments: {}",
                    tool.name,
                    tool.call_id,
                    tool.phase,
                    serde_json::to_string_pretty(&tool.arguments)
                        .unwrap_or_else(|_| tool.arguments.to_string())
                );
                if !tool.output.is_empty() {
                    let _ = write!(detail, "\noutput:\n{}", tool.output);
                    if tool.output_truncated {
                        detail.push_str("\n… output truncated");
                    }
                }
                if !tool.diff.is_empty() {
                    let _ = write!(detail, "\ndiff:\n{}", tool.diff);
                    if tool.diff_truncated {
                        detail.push_str("\n… diff truncated");
                    }
                }
            }
        }
        if !self.changed_paths.is_empty() {
            detail.push_str("\n\nCHANGED PATHS");
            for path in &self.changed_paths {
                let _ = write!(detail, "\n{}", path.display());
            }
        }
        if !self.response.trim().is_empty() {
            let _ = write!(detail, "\n\nRESPONSE\n{}", self.response.trim());
        }
        if let Some(error) = self.error_detail.as_ref().or(self.error.as_ref()) {
            let _ = write!(detail, "\n\nERROR DETAIL\n{error}");
        }
        detail
    }
}

#[derive(Clone, Copy)]
enum ToolClass {
    Read,
    Search,
    Run,
    Change,
    Other,
}

fn classify_tool(name: &str) -> ToolClass {
    let normalized = name.to_ascii_lowercase();
    if normalized.contains("search")
        || normalized.contains("grep")
        || normalized.contains("find")
        || normalized.contains("glob")
        || normalized.contains("web")
    {
        ToolClass::Search
    } else if normalized.contains("read")
        || normalized.contains("list")
        || normalized.contains("view")
        || normalized == "cat"
    {
        ToolClass::Read
    } else if normalized.contains("write")
        || normalized.contains("edit")
        || normalized.contains("patch")
        || normalized.contains("replace")
        || normalized.contains("delete")
    {
        ToolClass::Change
    } else if normalized.contains("shell")
        || normalized.contains("command")
        || normalized.contains("exec")
        || normalized.contains("run")
        || normalized.contains("terminal")
    {
        ToolClass::Run
    } else {
        ToolClass::Other
    }
}

fn active_tool_label(name: &str) -> String {
    match classify_tool(name) {
        ToolClass::Read => "Reading files".into(),
        ToolClass::Search if name.to_ascii_lowercase().contains("web") => {
            "Searching the web".into()
        }
        ToolClass::Search => "Searching the workspace".into(),
        ToolClass::Run => "Running a command".into(),
        ToolClass::Change => "Editing files".into(),
        ToolClass::Other => format!("Using {}", humanize_name(name)),
    }
}

fn completed_tool_label(name: &str) -> String {
    match classify_tool(name) {
        ToolClass::Read => "Read files".into(),
        ToolClass::Search if name.to_ascii_lowercase().contains("web") => "Searched the web".into(),
        ToolClass::Search => "Searched the workspace".into(),
        ToolClass::Run => "Ran a command".into(),
        ToolClass::Change => "Edited files".into(),
        ToolClass::Other => format!("Used {}", humanize_name(name)),
    }
}

fn humanize_name(name: &str) -> String {
    name.replace(['_', '-'], " ")
}

fn push_count(parts: &mut Vec<String>, count: usize, verb: &str, singular: &str, plural: &str) {
    if count > 0 {
        parts.push(format!(
            "{verb} {count} {}",
            if count == 1 { singular } else { plural }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_summary_folds_tools_into_human_verbs() {
        let mut task = TaskRunView::new(TurnId::new(), "verify".into());
        for (id, name) in [
            ("read-1", "read_file"),
            ("read-2", "list_files"),
            ("run-1", "shell"),
            ("edit-1", "apply_patch"),
        ] {
            task.ensure_tool(id, name, Value::Null).phase = ToolPhase::Completed;
        }
        assert_eq!(
            task.completion_summary(),
            "Read 2 items · Ran 1 command · Changed 1 file"
        );
    }

    #[test]
    fn tool_updates_reuse_the_call_identity() {
        let mut task = TaskRunView::new(TurnId::new(), "verify".into());
        task.ensure_tool("call", "tool", Value::Null);
        task.ensure_tool("call", "read_file", Value::Null).output = "README.md".into();
        assert_eq!(task.tools.len(), 1);
        assert_eq!(task.tools[0].name, "read_file");
    }
}
