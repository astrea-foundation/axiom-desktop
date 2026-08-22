use std::{
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{AxiomError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanState {
    Draft,
    Proposed,
    RevisionRequested,
    Approved,
    Abandoned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanComment {
    pub id: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanArtifact {
    pub id: String,
    pub revision: u64,
    pub state: PlanState,
    pub markdown: String,
    pub comments: Vec<PlanComment>,
}

impl PlanArtifact {
    #[must_use]
    pub fn new(markdown: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            revision: 1,
            state: PlanState::Draft,
            markdown: markdown.into(),
            comments: Vec::new(),
        }
    }

    pub fn propose(&mut self) -> Result<()> {
        self.transition(PlanState::Proposed)
    }

    pub fn approve(&mut self) -> Result<()> {
        self.transition(PlanState::Approved)
    }

    pub fn abandon(&mut self) -> Result<()> {
        self.transition(PlanState::Abandoned)
    }

    pub fn request_revision(
        &mut self,
        start_line: usize,
        end_line: usize,
        text: String,
    ) -> Result<()> {
        if self.state != PlanState::Proposed {
            return Err(AxiomError::InvalidTransition(
                "only a proposed plan can receive a revision request".into(),
            ));
        }
        self.comment(start_line, end_line, text)?;
        self.state = PlanState::RevisionRequested;
        Ok(())
    }

    pub fn revise(&mut self, expected_revision: u64, markdown: String) -> Result<()> {
        if self.revision != expected_revision {
            return Err(AxiomError::InvalidTransition(format!(
                "stale plan revision: expected {expected_revision}, current {}",
                self.revision
            )));
        }
        if matches!(self.state, PlanState::Approved | PlanState::Abandoned) {
            return Err(AxiomError::InvalidTransition(
                "terminal plan cannot be revised".into(),
            ));
        }
        validate_markdown(&markdown)?;
        self.markdown = markdown;
        self.revision = self.revision.saturating_add(1);
        self.state = PlanState::Draft;
        Ok(())
    }

    pub fn comment(&mut self, start_line: usize, end_line: usize, text: String) -> Result<()> {
        let line_count = self.markdown.lines().count().max(1);
        if text.trim().is_empty()
            || start_line == 0
            || end_line < start_line
            || end_line > line_count
        {
            return Err(AxiomError::InvalidTransition(
                "plan comment range is invalid".into(),
            ));
        }
        self.comments.push(PlanComment {
            id: Uuid::new_v4().to_string(),
            start_line,
            end_line,
            text,
        });
        Ok(())
    }

    pub fn save(&self, workspace: &Path) -> Result<PathBuf> {
        validate_markdown(&self.markdown)?;
        let root = workspace.canonicalize()?;
        let directory = root.join(".axiomcli/plans");
        std::fs::create_dir_all(&directory)?;
        let directory = directory.canonicalize()?;
        if !directory.starts_with(&root) {
            return Err(AxiomError::OutsideWorkspace(directory));
        }
        let path = directory.join(format!("{}.json", self.id));
        let temporary = directory.join(format!(".{}.{}.tmp", self.id, Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        Ok(path)
    }

    pub fn load(workspace: &Path, id: &str) -> Result<Self> {
        if !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err(AxiomError::OutsideWorkspace(PathBuf::from(id)));
        }
        let root = workspace.canonicalize()?;
        let path = root.join(".axiomcli/plans").join(format!("{id}.json"));
        let path = path.canonicalize()?;
        if !path.starts_with(&root) {
            return Err(AxiomError::OutsideWorkspace(path));
        }
        let plan: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        if plan.id != id {
            return Err(AxiomError::Storage(
                "plan file identity does not match its requested ID".into(),
            ));
        }
        validate_markdown(&plan.markdown)?;
        Ok(plan)
    }

    pub fn list(workspace: &Path) -> Result<Vec<Self>> {
        let root = workspace.canonicalize()?;
        let directory = root.join(".axiomcli/plans");
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let directory = directory.canonicalize()?;
        if !directory.starts_with(&root) {
            return Err(AxiomError::OutsideWorkspace(directory));
        }
        let mut plans = Vec::new();
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                let value: Self = serde_json::from_slice(&std::fs::read(entry.path())?)?;
                plans.push(value);
            }
        }
        plans.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(plans)
    }

    #[must_use]
    pub fn policy_path(workspace: &Path, id: Option<&str>) -> PathBuf {
        workspace
            .join(".axiomcli/plans")
            .join(format!("{}.json", id.unwrap_or("new-plan")))
    }

    fn transition(&mut self, target: PlanState) -> Result<()> {
        let allowed = matches!(
            (self.state, target),
            (PlanState::Draft, PlanState::Proposed)
                | (PlanState::Proposed, PlanState::Approved)
                | (
                    PlanState::Draft | PlanState::Proposed | PlanState::RevisionRequested,
                    PlanState::Abandoned
                )
        );
        if !allowed {
            return Err(AxiomError::InvalidTransition(format!(
                "plan cannot transition from {:?} to {target:?}",
                self.state
            )));
        }
        self.state = target;
        Ok(())
    }
}

fn validate_markdown(markdown: &str) -> Result<()> {
    if markdown.trim().is_empty() {
        return Err(AxiomError::InvalidTransition(
            "plan markdown cannot be empty".into(),
        ));
    }
    if markdown.len() > 1024 * 1024 {
        return Err(AxiomError::InvalidTransition(
            "plan markdown exceeds the 1 MiB development limit".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn plan_revisions_comments_and_terminal_state_are_durable() {
        let root = tempdir().expect("root");
        let mut plan = PlanArtifact::new("one\ntwo");
        plan.comment(2, 2, "change this".into()).expect("comment");
        plan.propose().expect("propose");
        plan.request_revision(2, 2, "be specific".into())
            .expect("request revision");
        plan.revise(1, "one\nrevised".into()).expect("revise");
        plan.propose().expect("propose again");
        plan.approve().expect("approve");
        plan.save(root.path()).expect("save");
        let restored = PlanArtifact::load(root.path(), &plan.id).expect("load");
        assert_eq!(restored, plan);
        assert!(plan.revise(2, "no".into()).is_err());
        assert_eq!(PlanArtifact::list(root.path()).expect("list"), vec![plan]);
    }
}
