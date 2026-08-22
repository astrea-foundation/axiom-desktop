use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{AxiomError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanState {
    Draft,
    Proposed,
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
        self.markdown = markdown;
        self.revision = self.revision.saturating_add(1);
        self.state = PlanState::Draft;
        Ok(())
    }

    pub fn comment(&mut self, start_line: usize, end_line: usize, text: String) -> Result<()> {
        let line_count = self.markdown.lines().count().max(1);
        if start_line == 0 || end_line < start_line || end_line > line_count {
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
        let root = workspace.canonicalize()?;
        let directory = root.join(".axiomcli/plans");
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{}.json", self.id));
        let temporary = directory.join(format!(".{}.{}.tmp", self.id, Uuid::new_v4()));
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
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
        serde_json::from_slice(&std::fs::read(path)?).map_err(Into::into)
    }

    fn transition(&mut self, target: PlanState) -> Result<()> {
        let allowed = matches!(
            (self.state, target),
            (PlanState::Draft, PlanState::Proposed)
                | (PlanState::Proposed, PlanState::Approved)
                | (PlanState::Draft | PlanState::Proposed, PlanState::Abandoned)
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
        plan.revise(1, "one\nrevised".into()).expect("revise");
        plan.propose().expect("propose again");
        plan.approve().expect("approve");
        plan.save(root.path()).expect("save");
        let restored = PlanArtifact::load(root.path(), &plan.id).expect("load");
        assert_eq!(restored, plan);
        assert!(plan.revise(2, "no".into()).is_err());
    }
}
