use std::{
    cell::RefCell,
    fmt::Write as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::{
    app::{TaskItem, TurnId},
    tool_display::{ToolDisplayPhase, describe_tool},
};

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
    Working(String),
    Waiting(String),
}

impl TaskActivity {
    pub(super) fn label(&self) -> String {
        match self {
            Self::Starting => "Starting the task".into(),
            Self::Thinking => "Thinking through the next step".into(),
            Self::Responding => "Writing the response".into(),
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
    Cancelled,
}

impl ToolPhase {
    pub(super) const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
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

    pub(super) fn timeline_label(&self, cwd: &Path) -> String {
        let label = describe_tool(
            &self.name,
            &self.arguments,
            match self.phase {
                ToolPhase::Proposed | ToolPhase::Cancelled => ToolDisplayPhase::Proposed,
                ToolPhase::Running => ToolDisplayPhase::Running,
                ToolPhase::Completed => ToolDisplayPhase::Completed,
                ToolPhase::Failed => ToolDisplayPhase::Failed,
            },
            Some(cwd),
        );
        if self.phase == ToolPhase::Cancelled {
            format!("Stopped · {label}")
        } else {
            label
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum TaskTimelineItem {
    Response {
        text: String,
        markdown: Box<RefCell<StreamingMarkdownRenderer>>,
    },
    Tool {
        call_id: String,
    },
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
    pub(super) timeline: Vec<TaskTimelineItem>,
    pub(super) tasks: Vec<TaskItem>,
    pub(super) changed_paths: Vec<PathBuf>,
    pub(super) error: Option<String>,
    pub(super) error_detail: Option<String>,
    pub(super) started_at: Instant,
    pub(super) finished_at: Option<Instant>,
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
            timeline: Vec::new(),
            tasks: Vec::new(),
            changed_paths: Vec::new(),
            error: None,
            error_detail: None,
            started_at: Instant::now(),
            finished_at: None,
        }
    }

    pub(super) fn finish(&mut self, phase: TaskPhase, error: Option<String>) {
        self.phase = phase;
        if phase == TaskPhase::Cancelled {
            for tool in &mut self.tools {
                if !tool.phase.is_terminal() {
                    tool.phase = ToolPhase::Cancelled;
                }
            }
        }
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

    pub(super) fn append_response(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.response.push_str(text);
        if let Some(TaskTimelineItem::Response { text: response, .. }) = self.timeline.last_mut() {
            response.push_str(text);
        } else {
            self.timeline.push(TaskTimelineItem::Response {
                text: text.to_owned(),
                markdown: Box::new(RefCell::default()),
            });
        }
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
        self.timeline.push(TaskTimelineItem::Tool {
            call_id: call_id.to_owned(),
        });
        self.tools.last_mut().expect("tool was just inserted")
    }

    pub(super) fn running_tool(&self) -> Option<&ToolView> {
        self.tools
            .iter()
            .rev()
            .find(|tool| !tool.phase.is_terminal())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_and_tools_keep_their_chronological_positions() {
        let mut task = TaskRunView::new(TurnId::new(), "verify".into());
        task.append_response("I’ll read it.");
        task.ensure_tool(
            "read-1",
            "read_file",
            serde_json::json!({"path": "README.md"}),
        )
        .phase = ToolPhase::Completed;
        task.append_response("Here is the answer.");

        assert!(matches!(
            task.timeline[0],
            TaskTimelineItem::Response { .. }
        ));
        assert!(matches!(task.timeline[1], TaskTimelineItem::Tool { .. }));
        assert!(matches!(
            task.timeline[2],
            TaskTimelineItem::Response { .. }
        ));
        assert_eq!(
            task.tools[0].timeline_label(Path::new("/work")),
            "Read `README.md`"
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
