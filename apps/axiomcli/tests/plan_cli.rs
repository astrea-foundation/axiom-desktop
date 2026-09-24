use std::process::Command;

use axiomcli::planning::{PlanArtifact, PlanState};
use tempfile::TempDir;

fn plans(root: &TempDir, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_axiomcli"))
        .env("AXIOMCLI_TEST_RUNNER", "echo")
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env_remove("AXIOM_API_KEY")
        .arg("plans")
        .arg("--cwd")
        .arg(root.path())
        .args(args)
        .output()
        .expect("run plans command")
}

#[test]
fn plan_review_commands_persist_comments_revisions_and_terminal_state() {
    let root = TempDir::new().expect("workspace");
    let plan = PlanArtifact::new("# Plan\n\nInspect");
    let id = plan.id.clone();
    plan.save(root.path()).expect("save fixture");

    assert!(plans(&root, &["propose", &id]).status.success());
    assert!(
        plans(&root, &["request-revision", &id, "3", "3", "Add tests"])
            .status
            .success()
    );
    let requested = PlanArtifact::load(root.path(), &id).expect("requested");
    assert_eq!(requested.state, PlanState::RevisionRequested);
    assert_eq!(requested.comments.len(), 1);

    assert!(
        plans(&root, &["revise", &id, "1", "# Plan\n\nInspect\nTest"])
            .status
            .success()
    );
    assert!(
        plans(&root, &["comment", &id, "4", "4", "Looks good"])
            .status
            .success()
    );
    assert!(plans(&root, &["propose", &id]).status.success());
    assert!(plans(&root, &["approve", &id]).status.success());

    let approved = PlanArtifact::load(root.path(), &id).expect("approved");
    assert_eq!(approved.state, PlanState::Approved);
    assert_eq!(approved.revision, 2);
    assert_eq!(approved.comments.len(), 2);
    let shown = plans(&root, &["show", &id]);
    assert!(shown.status.success());
    assert!(
        String::from_utf8(shown.stdout)
            .expect("utf8")
            .contains("approved")
    );
}
