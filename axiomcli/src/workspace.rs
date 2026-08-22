use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::Read as _,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use git2::{DiffFormat, Repository, StatusOptions};
use globset::Glob;
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
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileMetadata {
    pub path: PathBuf,
    pub kind: String,
    pub size_bytes: u64,
    pub readonly: bool,
    pub executable: bool,
    pub content: ContentClassification,
    pub provenance: SourceClassification,
    pub modified_unix_seconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentClassification {
    Text,
    Binary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceClassification {
    Authored,
    LikelyGenerated,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextRange {
    pub path: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
    pub sha256: String,
    pub score: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitStatusEntry {
    pub path: PathBuf,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitCommitSummary {
    pub id: String,
    pub summary: String,
    pub author: String,
    pub unix_seconds: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitInspection {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub status: Vec<GitStatusEntry>,
    pub recent_commits: Vec<GitCommitSummary>,
    pub diff: String,
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

    /// Discover the nearest repository root, falling back to the supplied
    /// directory when it is not inside a Git worktree.
    pub fn discover(start: impl AsRef<Path>) -> Result<Self> {
        let start = start.as_ref().canonicalize()?;
        let directory = if start.is_dir() {
            start
        } else {
            start
                .parent()
                .ok_or_else(|| AxiomError::Config("workspace path has no parent".into()))?
                .to_path_buf()
        };
        let root = directory
            .ancestors()
            .find(|ancestor| ancestor.join(".git").exists())
            .unwrap_or(&directory)
            .to_path_buf();
        Self::new(root)
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

    pub fn glob(
        &self,
        pattern: &str,
        max_entries: usize,
        include_ignored: bool,
    ) -> Result<Vec<PathBuf>> {
        if pattern.trim().is_empty() {
            return Err(AxiomError::Tool("glob pattern cannot be empty".into()));
        }
        let matcher = Glob::new(pattern)
            .map_err(|error| AxiomError::Tool(format!("invalid glob pattern: {error}")))?
            .compile_matcher();
        let mut builder = WalkBuilder::new(&self.root);
        builder
            .hidden(false)
            .git_ignore(!include_ignored)
            .git_global(!include_ignored)
            .git_exclude(!include_ignored)
            .ignore(!include_ignored);
        let mut paths = Vec::new();
        for entry in builder.build().filter_map(std::result::Result::ok) {
            let relative = self.relative(entry.path());
            if relative.as_os_str().is_empty()
                || relative.starts_with(".git")
                || !matcher.is_match(&relative)
            {
                continue;
            }
            paths.push(relative);
            if paths.len() >= max_entries {
                break;
            }
        }
        paths.sort();
        Ok(paths)
    }

    pub fn metadata(&self, path: impl AsRef<Path>) -> Result<FileMetadata> {
        let input = path.as_ref();
        let candidate = self.candidate(input)?;
        let link_metadata = std::fs::symlink_metadata(&candidate)?;
        let resolved = self.resolve_existing(&candidate)?;
        let metadata = std::fs::metadata(&resolved)?;
        let kind = if link_metadata.file_type().is_symlink() {
            "symlink"
        } else if metadata.is_file() {
            "file"
        } else if metadata.is_dir() {
            "directory"
        } else {
            "special"
        };
        let prefix = if metadata.is_file() {
            read_verified_prefix(&resolved, &self.root, 8192)?
        } else {
            Vec::new()
        };
        Ok(FileMetadata {
            path: self.relative(&candidate),
            kind: kind.into(),
            size_bytes: metadata.len(),
            readonly: metadata.permissions().readonly(),
            executable: is_executable(&metadata),
            content: if looks_binary(&prefix) {
                ContentClassification::Binary
            } else {
                ContentClassification::Text
            },
            provenance: if likely_generated(&self.relative(&candidate), &prefix) {
                SourceClassification::LikelyGenerated
            } else {
                SourceClassification::Authored
            },
            modified_unix_seconds: metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|value| value.as_secs()),
        })
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
            let relative = self.relative(entry.path());
            let Ok(read) = self.read(
                &relative,
                1,
                100_000,
                usize::try_from(max_file_bytes).unwrap_or(usize::MAX),
            ) else {
                continue;
            };
            for (index, line) in read.content.lines().enumerate() {
                if line.contains(query) {
                    matches.push(format!(
                        "{}:{}:{}",
                        relative.display(),
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

    pub fn select_context(
        &self,
        query: &str,
        max_files: usize,
        max_bytes: usize,
    ) -> Result<Vec<ContextRange>> {
        if query.trim().is_empty() {
            return Err(AxiomError::Tool("context query cannot be empty".into()));
        }
        let needle = query.to_lowercase();
        let mut candidates = Vec::new();
        let walker = WalkBuilder::new(&self.root)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .build();
        for entry in walker.filter_map(std::result::Result::ok) {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() || metadata.len() > 2 * 1024 * 1024 {
                continue;
            }
            let relative = self.relative(entry.path());
            let Ok(read) = self.read(&relative, 1, 100_000, 2 * 1024 * 1024) else {
                continue;
            };
            let lines: Vec<_> = read.content.lines().collect();
            let matches: Vec<_> = lines
                .iter()
                .enumerate()
                .filter_map(|(index, line)| line.to_lowercase().contains(&needle).then_some(index))
                .collect();
            if matches.is_empty() {
                continue;
            }
            let first = matches[0];
            let start = first.saturating_sub(3);
            let end = (matches[matches.len() - 1] + 4).min(lines.len());
            let content = lines[start..end].join("\n");
            let name_bonus =
                usize::from(relative.to_string_lossy().to_lowercase().contains(&needle));
            candidates.push(ContextRange {
                path: relative,
                start_line: start + 1,
                end_line: end,
                content,
                sha256: read.sha256,
                score: matches
                    .len()
                    .saturating_mul(10)
                    .saturating_add(name_bonus * 25),
            });
        }
        candidates.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.path.cmp(&right.path))
        });
        let mut retained = Vec::new();
        let mut retained_bytes = 0_usize;
        for mut range in candidates.into_iter().take(max_files) {
            let remaining = max_bytes.saturating_sub(retained_bytes);
            if remaining == 0 {
                break;
            }
            let mut truncated = false;
            range.content = truncate_utf8(&range.content, remaining, &mut truncated);
            range.end_line = range
                .start_line
                .saturating_add(range.content.lines().count().saturating_sub(1));
            retained_bytes = retained_bytes.saturating_add(range.content.len());
            retained.push(range);
            if truncated {
                break;
            }
        }
        Ok(retained)
    }

    pub fn inspect_git(
        &self,
        max_status: usize,
        max_commits: usize,
        max_diff_bytes: usize,
    ) -> Result<GitInspection> {
        let repository = Repository::discover(&self.root)
            .map_err(|error| AxiomError::Tool(format!("not a Git repository: {error}")))?;
        let workdir = repository
            .workdir()
            .ok_or_else(|| AxiomError::Tool("bare Git repositories are not supported".into()))?
            .canonicalize()?;
        if self.root != workdir {
            return Err(AxiomError::OutsideWorkspace(workdir));
        }
        let head = repository.head().ok();
        let branch = head
            .as_ref()
            .and_then(|head| head.shorthand().ok())
            .map(str::to_owned);
        let head_id = head
            .as_ref()
            .and_then(git2::Reference::target)
            .map(|id| id.to_string());

        let mut options = StatusOptions::new();
        options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(false);
        let statuses = repository
            .statuses(Some(&mut options))?
            .iter()
            .take(max_status)
            .map(|entry| GitStatusEntry {
                path: PathBuf::from(entry.path().unwrap_or("(non-utf8 path)")),
                status: format!("{:?}", entry.status()),
            })
            .collect();

        let mut recent_commits = Vec::new();
        if let Ok(mut walk) = repository.revwalk()
            && walk.push_head().is_ok()
        {
            for id in walk.take(max_commits).filter_map(std::result::Result::ok) {
                let Ok(commit) = repository.find_commit(id) else {
                    continue;
                };
                recent_commits.push(GitCommitSummary {
                    id: id.to_string(),
                    summary: commit
                        .summary()
                        .ok()
                        .flatten()
                        .unwrap_or("(no summary)")
                        .to_owned(),
                    author: commit.author().name().unwrap_or("unknown").to_owned(),
                    unix_seconds: commit.time().seconds(),
                });
            }
        }

        let diff = repository.diff_index_to_workdir(None, None)?;
        let output = std::cell::RefCell::new(Vec::new());
        let truncated = std::cell::Cell::new(false);
        diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
            let mut output = output.borrow_mut();
            let remaining = max_diff_bytes.saturating_sub(output.len());
            if remaining == 0 {
                truncated.set(true);
                return true;
            }
            let content = line.content();
            output.extend_from_slice(&content[..content.len().min(remaining)]);
            truncated.set(truncated.get() || content.len() > remaining);
            true
        })?;
        let diff = String::from_utf8_lossy(&output.into_inner()).into_owned();
        Ok(GitInspection {
            branch,
            head: head_id,
            status: statuses,
            recent_commits,
            diff,
            truncated: truncated.get(),
        })
    }

    pub fn read(
        &self,
        path: impl AsRef<Path>,
        start_line: usize,
        max_lines: usize,
        max_bytes: usize,
    ) -> Result<FileRead> {
        const MAX_INSPECT_BYTES: u64 = 8 * 1024 * 1024;
        let path = self.resolve_existing(path)?;
        let mut file = open_verified(&path, &self.root)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(AxiomError::Tool(format!(
                "special file cannot be read: {}",
                self.relative(&path).display()
            )));
        }
        if metadata.len() > MAX_INSPECT_BYTES {
            return Err(AxiomError::Tool(format!(
                "large file omitted: {} is {} bytes (limit {MAX_INSPECT_BYTES}); inspect metadata or select a smaller source file",
                self.relative(&path).display(),
                metadata.len()
            )));
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
        file.read_to_end(&mut bytes)?;
        if bytes.iter().take(8192).any(|byte| *byte == 0) {
            return Err(AxiomError::Tool(format!(
                "binary file cannot be read as text: {}",
                self.relative(&path).display()
            )));
        }
        let sha256 = digest(&bytes);
        let full = std::str::from_utf8(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
            .map_err(|_| {
            AxiomError::Tool(format!(
                "text file is not valid UTF-8: {}",
                self.relative(&path).display()
            ))
        })?;
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
            size_bytes: metadata.len(),
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

fn open_verified(path: &Path, root: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd as _;
        let actual = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())).canonicalize()?;
        if !actual.starts_with(root) {
            return Err(AxiomError::OutsideWorkspace(actual));
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let actual = path.canonicalize()?;
        if !actual.starts_with(root) {
            return Err(AxiomError::OutsideWorkspace(actual));
        }
    }
    Ok(file)
}

fn read_verified_prefix(path: &Path, root: &Path, max_bytes: usize) -> Result<Vec<u8>> {
    let file = open_verified(path, root)?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(max_bytes).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
        || (!bytes.is_empty()
            && bytes
                .iter()
                .filter(|byte| byte.is_ascii_control() && !matches!(byte, b'\n' | b'\r' | b'\t'))
                .count()
                .saturating_mul(20)
                > bytes.len())
}

fn likely_generated(path: &Path, prefix: &[u8]) -> bool {
    let generated_directory = path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some("target" | "node_modules" | "dist" | "build" | "vendor" | "generated")
        )
    });
    let header = String::from_utf8_lossy(prefix).to_lowercase();
    generated_directory
        || header.lines().take(5).any(|line| {
            line.contains("generated file")
                || line.contains("code generated")
                || line.contains("do not edit")
        })
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    false
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
    use git2::{IndexAddOption, Signature};
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

    #[test]
    fn discovery_glob_metadata_and_context_are_bounded_and_stable() {
        let root = tempdir().expect("root");
        Repository::init(root.path()).expect("repository");
        std::fs::create_dir_all(root.path().join("src/nested")).expect("src");
        std::fs::write(root.path().join(".gitignore"), "ignored.txt\n").expect("ignore");
        std::fs::write(
            root.path().join("src/lib.rs"),
            "pub fn cherry_router() {\n    // cherry context\n}\n",
        )
        .expect("rust");
        std::fs::write(
            root.path().join("src/nested/app.py"),
            "def cherry_worker():\n    return 'context'\n",
        )
        .expect("python");
        std::fs::write(root.path().join("ignored.txt"), "cherry secret\n").expect("ignored");
        std::fs::write(
            root.path().join("generated.rs"),
            "// Code generated; DO NOT EDIT.\npub const VALUE: u8 = 1;\n",
        )
        .expect("generated");
        let discovered = Workspace::discover(root.path().join("src/nested")).expect("discover");
        assert_eq!(discovered.root(), root.path().canonicalize().expect("root"));
        assert_eq!(
            discovered.glob("**/*.rs", 10, false).expect("glob"),
            vec![PathBuf::from("generated.rs"), PathBuf::from("src/lib.rs")]
        );
        assert!(
            !discovered
                .glob("**/*.txt", 10, false)
                .expect("ignored glob")
                .contains(&PathBuf::from("ignored.txt"))
        );
        assert!(
            discovered
                .glob("**/*.txt", 10, true)
                .expect("override glob")
                .contains(&PathBuf::from("ignored.txt"))
        );
        let metadata = discovered.metadata("generated.rs").expect("metadata");
        assert_eq!(metadata.provenance, SourceClassification::LikelyGenerated);
        assert_eq!(metadata.content, ContentClassification::Text);

        let first = discovered
            .select_context("cherry", 8, 120)
            .expect("context");
        let second = discovered
            .select_context("cherry", 8, 120)
            .expect("stable context");
        assert_eq!(
            serde_json::to_string(&first).expect("first"),
            serde_json::to_string(&second).expect("second")
        );
        assert!(first.iter().all(|range| !range.path.is_absolute()));
        assert!(first.iter().map(|range| range.content.len()).sum::<usize>() <= 120);
        assert!(
            first
                .iter()
                .any(|range| range.path == Path::new("src/lib.rs"))
        );
    }

    #[test]
    fn binary_large_invalid_encoding_special_and_symlink_swap_fail_safely() {
        let root = tempdir().expect("root");
        let outside = tempdir().expect("outside");
        std::fs::write(root.path().join("binary.bin"), b"abc\0def").expect("binary");
        std::fs::write(root.path().join("invalid.txt"), [0xff, 0xfe, 0x41]).expect("encoding");
        let large = File::create(root.path().join("large.txt")).expect("large");
        large.set_len(9 * 1024 * 1024).expect("sparse length");
        std::fs::write(root.path().join("victim.txt"), "inside").expect("victim");
        std::fs::write(outside.path().join("secret.txt"), "outside").expect("outside");
        let workspace = Workspace::new(root.path()).expect("workspace");
        assert!(workspace.read("binary.bin", 1, 10, 100).is_err());
        assert!(workspace.read("invalid.txt", 1, 10, 100).is_err());
        let large_error = workspace
            .read("large.txt", 1, 10, 100)
            .expect_err("large file");
        assert!(large_error.to_string().contains("large file omitted"));

        #[cfg(unix)]
        {
            use std::os::unix::{fs::symlink, net::UnixListener};
            let socket_path = root.path().join("special.sock");
            let _socket = UnixListener::bind(&socket_path).expect("socket");
            assert!(workspace.read("special.sock", 1, 10, 100).is_err());

            let _checked = workspace
                .resolve_existing("victim.txt")
                .expect("policy check");
            std::fs::remove_file(root.path().join("victim.txt")).expect("remove victim");
            symlink(
                outside.path().join("secret.txt"),
                root.path().join("victim.txt"),
            )
            .expect("swap");
            assert!(workspace.read("victim.txt", 1, 10, 100).is_err());
        }
    }

    #[test]
    fn git_inspection_uses_library_and_returns_bounded_status_diff_and_log() {
        let root = tempdir().expect("root");
        let repository = Repository::init(root.path()).expect("repository");
        std::fs::write(root.path().join("tracked.txt"), "old\n").expect("tracked");
        let mut index = repository.index().expect("index");
        index
            .add_all(["tracked.txt"], IndexAddOption::DEFAULT, None)
            .expect("add");
        let tree_id = index.write_tree().expect("tree id");
        let tree = repository.find_tree(tree_id).expect("tree");
        let signature = Signature::now("AxiomCLI test", "test@example.invalid").expect("signature");
        repository
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "initial fixture",
                &tree,
                &[],
            )
            .expect("commit");
        drop(tree);
        std::fs::write(root.path().join("tracked.txt"), "new\n").expect("modify");
        std::fs::write(root.path().join("untracked.txt"), "new\n").expect("untracked");
        let workspace = Workspace::new(root.path()).expect("workspace");
        let inspection = workspace.inspect_git(10, 5, 32).expect("inspect");
        assert!(inspection.head.is_some());
        assert_eq!(inspection.recent_commits[0].summary, "initial fixture");
        assert!(
            inspection
                .status
                .iter()
                .any(|entry| entry.path == Path::new("tracked.txt"))
        );
        assert!(inspection.diff.len() <= 32);
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
