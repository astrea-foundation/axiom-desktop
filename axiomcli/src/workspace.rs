use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use similar::{ChangeTag, TextDiff};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _},
    process::Command,
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{AxiomError, Result};

#[derive(Clone, Debug)]
pub struct Workspace {
    root: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileRead {
    pub path: PathBuf,
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
    pub sha256: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PatchOutcome {
    pub path: PathBuf,
    pub old_sha256: Option<String>,
    pub new_sha256: String,
    pub diff: String,
}

impl Workspace {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(AxiomError::Config(format!(
                "workspace is not a directory: {}",
                root.display()
            )));
        }
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn resolve_existing(&self, input: impl AsRef<Path>) -> Result<PathBuf> {
        let candidate = self.candidate(input.as_ref())?;
        let canonical = candidate.canonicalize()?;
        self.ensure_inside(canonical)
    }

    pub fn resolve_for_write(&self, input: impl AsRef<Path>) -> Result<PathBuf> {
        let candidate = self.candidate(input.as_ref())?;
        if candidate.exists() {
            return self.resolve_existing(candidate);
        }
        let parent = candidate
            .parent()
            .ok_or_else(|| AxiomError::OutsideWorkspace(candidate.clone()))?;
        let parent = parent.canonicalize()?;
        let parent = self.ensure_inside(parent)?;
        let name = candidate
            .file_name()
            .ok_or_else(|| AxiomError::OutsideWorkspace(candidate.clone()))?;
        Ok(parent.join(name))
    }

    pub fn list(&self, path: impl AsRef<Path>, max_entries: usize) -> Result<Vec<PathBuf>> {
        let directory = self.resolve_existing(path)?;
        let mut entries = std::fs::read_dir(directory)?
            .take(max_entries.saturating_add(1))
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        if entries.len() > max_entries {
            entries.truncate(max_entries);
        }
        entries.sort();
        Ok(entries
            .into_iter()
            .map(|path| self.relative(&path))
            .collect())
    }

    pub fn search(
        &self,
        query: &str,
        max_matches: usize,
        max_file_bytes: u64,
    ) -> Result<Vec<String>> {
        if query.is_empty() {
            return Err(AxiomError::Tool("search query cannot be empty".into()));
        }
        let mut matches = Vec::new();
        let walker = WalkBuilder::new(&self.root)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .build();
        for entry in walker.filter_map(std::result::Result::ok) {
            if matches.len() >= max_matches {
                break;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) if metadata.is_file() && metadata.len() <= max_file_bytes => metadata,
                _ => continue,
            };
            let _ = metadata;
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            for (index, line) in content.lines().enumerate() {
                if line.contains(query) {
                    matches.push(format!(
                        "{}:{}:{}",
                        self.relative(entry.path()).display(),
                        index + 1,
                        line.trim()
                    ));
                    if matches.len() >= max_matches {
                        break;
                    }
                }
            }
        }
        Ok(matches)
    }

    pub fn read(
        &self,
        path: impl AsRef<Path>,
        start_line: usize,
        max_lines: usize,
        max_bytes: usize,
    ) -> Result<FileRead> {
        let path = self.resolve_existing(path)?;
        let bytes = std::fs::read(&path)?;
        if bytes.iter().take(8192).any(|byte| *byte == 0) {
            return Err(AxiomError::Tool(format!(
                "binary file cannot be read as text: {}",
                self.relative(&path).display()
            )));
        }
        let sha256 = digest(&bytes);
        let full = String::from_utf8_lossy(&bytes);
        let start_line = start_line.max(1);
        let lines: Vec<_> = full
            .lines()
            .skip(start_line - 1)
            .take(max_lines.saturating_add(1))
            .collect();
        let mut truncated = lines.len() > max_lines;
        let selected = lines
            .into_iter()
            .take(max_lines)
            .collect::<Vec<_>>()
            .join("\n");
        let content = truncate_utf8(&selected, max_bytes, &mut truncated);
        let line_count = content.lines().count();
        Ok(FileRead {
            path: self.relative(&path),
            content,
            start_line,
            end_line: start_line.saturating_add(line_count.saturating_sub(1)),
            sha256,
            truncated,
        })
    }

    pub fn apply_replacement(
        &self,
        path: impl AsRef<Path>,
        old: &str,
        new: &str,
        expected_sha256: Option<&str>,
    ) -> Result<PatchOutcome> {
        let path = self.resolve_for_write(path)?;
        let (old_content, old_permissions, old_sha256) = if path.exists() {
            let bytes = std::fs::read(&path)?;
            let sha = digest(&bytes);
            if let Some(expected) = expected_sha256
                && expected != sha
            {
                return Err(AxiomError::Tool(format!(
                    "stale edit: expected {expected}, found {sha}"
                )));
            }
            (
                String::from_utf8(bytes)
                    .map_err(|_| AxiomError::Tool("cannot edit a non-UTF-8 file".into()))?,
                Some(std::fs::metadata(&path)?.permissions()),
                Some(sha),
            )
        } else {
            if !old.is_empty() {
                return Err(AxiomError::Tool(
                    "new file replacement must use an empty old string".into(),
                ));
            }
            (String::new(), None, None)
        };

        let new_content = if old.is_empty() {
            if old_content.is_empty() {
                new.to_owned()
            } else {
                return Err(AxiomError::Tool(
                    "empty replacement is allowed only for a new/empty file".into(),
                ));
            }
        } else {
            let count = old_content.matches(old).count();
            if count != 1 {
                return Err(AxiomError::Tool(format!(
                    "replacement must match exactly once; found {count} matches"
                )));
            }
            old_content.replacen(old, new, 1)
        };

        let temp = path.with_file_name(format!(
            ".{}.axiomcli-{}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("edit"),
            Uuid::new_v4()
        ));
        std::fs::write(&temp, new_content.as_bytes())?;
        if let Some(permissions) = old_permissions {
            std::fs::set_permissions(&temp, permissions)?;
        }
        if let Err(error) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(error.into());
        }

        Ok(PatchOutcome {
            path: self.relative(&path),
            old_sha256,
            new_sha256: digest(new_content.as_bytes()),
            diff: unified_diff(&old_content, &new_content),
        })
    }

    fn candidate(&self, input: &Path) -> Result<PathBuf> {
        if input
            .components()
            .any(|component| component == Component::ParentDir)
        {
            return Err(AxiomError::OutsideWorkspace(input.to_path_buf()));
        }
        Ok(if input.is_absolute() {
            input.to_path_buf()
        } else {
            self.root.join(input)
        })
    }

    fn ensure_inside(&self, path: PathBuf) -> Result<PathBuf> {
        if path.starts_with(&self.root) {
            Ok(path)
        } else {
            Err(AxiomError::OutsideWorkspace(path))
        }
    }

    fn relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.root).unwrap_or(path).to_path_buf()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessRequest {
    pub program: String,
    pub args: Vec<String>,
    pub timeout_secs: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessOutcome {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackgroundSnapshot {
    pub id: String,
    pub state: String,
    pub outcome: Option<ProcessOutcome>,
}

#[derive(Clone)]
struct BackgroundTask {
    cancellation: CancellationToken,
    state: Arc<Mutex<BackgroundSnapshot>>,
}

#[derive(Clone, Default)]
pub struct ProcessManager {
    tasks: Arc<Mutex<HashMap<String, BackgroundTask>>>,
    max_output_bytes: usize,
}

impl ProcessManager {
    #[must_use]
    pub fn new(max_output_bytes: usize) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            max_output_bytes: max_output_bytes.max(1024),
        }
    }

    pub async fn run(
        &self,
        workspace: &Workspace,
        request: &ProcessRequest,
        cancellation: CancellationToken,
    ) -> Result<ProcessOutcome> {
        if request.program.trim().is_empty() {
            return Err(AxiomError::Tool("program cannot be empty".into()));
        }
        let mut command = Command::new(&request.program);
        command
            .args(&request.args)
            .current_dir(workspace.root())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env_clear();
        for variable in ["PATH", "LANG", "LC_ALL", "TERM"] {
            if let Ok(value) = std::env::var(variable) {
                command.env(variable, value);
            }
        }
        #[cfg(unix)]
        {
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        let pid = child.id();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AxiomError::Tool("missing stdout pipe".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AxiomError::Tool("missing stderr pipe".into()))?;
        let max = self.max_output_bytes;
        let stdout_task = tokio::spawn(read_bounded(stdout, max));
        let stderr_task = tokio::spawn(read_bounded(stderr, max));
        let timeout = Duration::from_secs(request.timeout_secs.clamp(1, 3600));
        let status = tokio::select! {
            () = cancellation.cancelled() => {
                terminate_process_group(pid, &mut child).await;
                return Err(AxiomError::Cancelled);
            }
            result = tokio::time::timeout(timeout, child.wait()) => if let Ok(status) = result {
                status?
            } else {
                terminate_process_group(pid, &mut child).await;
                return Err(AxiomError::Tool(format!("process timed out after {} seconds", request.timeout_secs)));
            }
        };
        let (stdout, stdout_truncated) = stdout_task
            .await
            .map_err(|error| AxiomError::Tool(error.to_string()))??;
        let (stderr, stderr_truncated) = stderr_task
            .await
            .map_err(|error| AxiomError::Tool(error.to_string()))??;
        Ok(ProcessOutcome {
            exit_code: status.code(),
            stdout,
            stderr,
            truncated: stdout_truncated || stderr_truncated,
        })
    }

    pub async fn start(&self, workspace: Workspace, request: ProcessRequest) -> String {
        let id = Uuid::new_v4().to_string();
        let cancellation = CancellationToken::new();
        let state = Arc::new(Mutex::new(BackgroundSnapshot {
            id: id.clone(),
            state: "running".into(),
            outcome: None,
        }));
        self.tasks.lock().await.insert(
            id.clone(),
            BackgroundTask {
                cancellation: cancellation.clone(),
                state: state.clone(),
            },
        );
        let manager = self.clone();
        tokio::spawn(async move {
            let outcome = manager.run(&workspace, &request, cancellation).await;
            let mut snapshot = state.lock().await;
            match outcome {
                Ok(outcome) => {
                    snapshot.state = "completed".into();
                    snapshot.outcome = Some(outcome);
                }
                Err(AxiomError::Cancelled) => snapshot.state = "cancelled".into(),
                Err(error) => {
                    snapshot.state = format!("failed: {error}");
                }
            }
        });
        id
    }

    pub async fn snapshot(&self, id: &str) -> Result<BackgroundSnapshot> {
        let state = self
            .tasks
            .lock()
            .await
            .get(id)
            .map(|task| task.state.clone())
            .ok_or_else(|| AxiomError::Tool(format!("unknown background task `{id}`")))?;
        let snapshot = state.lock().await.clone();
        Ok(snapshot)
    }

    pub async fn stop(&self, id: &str) -> Result<()> {
        let task = self
            .tasks
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| AxiomError::Tool(format!("unknown background task `{id}`")))?;
        task.cancellation.cancel();
        Ok(())
    }
}

async fn read_bounded(
    mut reader: impl AsyncRead + Unpin,
    max_bytes: usize,
) -> Result<(String, bool)> {
    let mut retained = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..read.min(remaining)]);
        truncated |= read > remaining;
    }
    let text = String::from_utf8_lossy(&retained)
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect();
    Ok((text, truncated))
}

async fn terminate_process_group(pid: Option<u32>, child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGTERM,
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn truncate_utf8(input: &str, max_bytes: usize, truncated: &mut bool) -> String {
    if input.len() <= max_bytes {
        return input.into();
    }
    let mut end = max_bytes;
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    *truncated = true;
    input[..end].into()
}

fn unified_diff(old: &str, new: &str) -> String {
    let mut output = String::new();
    for change in TextDiff::from_lines(old, new).iter_all_changes() {
        let marker = match change.tag() {
            ChangeTag::Delete => '-',
            ChangeTag::Insert => '+',
            ChangeTag::Equal => ' ',
        };
        output.push(marker);
        output.push_str(change.value());
    }
    output
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn traversal_and_symlink_escape_are_rejected() {
        let root = tempdir().expect("root");
        let outside = tempdir().expect("outside");
        let workspace = Workspace::new(root.path()).expect("workspace");
        assert!(workspace.resolve_for_write("../escape").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), root.path().join("link")).expect("symlink");
            assert!(workspace.resolve_existing("link").is_err());
        }
    }

    #[test]
    fn replacement_is_atomic_and_hash_checked() {
        let root = tempdir().expect("root");
        std::fs::write(root.path().join("file.txt"), "old\n").expect("fixture");
        let workspace = Workspace::new(root.path()).expect("workspace");
        let read = workspace.read("file.txt", 1, 100, 1000).expect("read");
        let outcome = workspace
            .apply_replacement("file.txt", "old", "new", Some(&read.sha256))
            .expect("patch");
        assert!(outcome.diff.contains("-old"));
        assert_eq!(
            std::fs::read_to_string(root.path().join("file.txt")).expect("result"),
            "new\n"
        );
        assert!(
            workspace
                .apply_replacement("file.txt", "new", "again", Some(&read.sha256))
                .is_err()
        );
    }

    #[tokio::test]
    async fn process_output_is_bounded() {
        let root = tempdir().expect("root");
        let workspace = Workspace::new(root.path()).expect("workspace");
        let manager = ProcessManager::new(1024);
        let outcome = manager
            .run(
                &workspace,
                &ProcessRequest {
                    program: "sh".into(),
                    args: vec!["-c".into(), "yes x | head -c 4096".into()],
                    timeout_secs: 5,
                },
                CancellationToken::new(),
            )
            .await
            .expect("process");
        assert!(outcome.truncated);
        assert_eq!(outcome.stdout.len(), 1024);

        let escaped = manager
            .run(
                &workspace,
                &ProcessRequest {
                    program: "printf".into(),
                    args: vec!["\u{1b}[31mred\n".into()],
                    timeout_secs: 5,
                },
                CancellationToken::new(),
            )
            .await
            .expect("escaped process");
        assert!(!escaped.stdout.contains('\u{1b}'));
    }
}
