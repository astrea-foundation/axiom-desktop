use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
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
    sync::{Mutex, watch},
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredPatch {
    pub edits: Vec<EditIntent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "operation", deny_unknown_fields)]
pub enum EditIntent {
    Create {
        path: PathBuf,
        content: String,
        #[serde(default)]
        mode: Option<u32>,
    },
    Update {
        path: PathBuf,
        expected_sha256: String,
        content: String,
        #[serde(default = "default_true")]
        preserve_line_endings: bool,
    },
    Delete {
        path: PathBuf,
        expected_sha256: String,
    },
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StructuredEditOutcome {
    pub operation: String,
    pub path: PathBuf,
    pub old_sha256: Option<String>,
    pub new_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PatchTransactionOutcome {
    pub edits: Vec<StructuredEditOutcome>,
    pub changed_paths: Vec<PathBuf>,
    pub diff: String,
    pub diff_truncated: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreparedOperation {
    Create,
    Update,
    Delete,
}

struct PreparedEdit {
    operation: PreparedOperation,
    path: PathBuf,
    relative: PathBuf,
    expected_sha256: Option<String>,
    old_sha256: Option<String>,
    new_sha256: Option<String>,
    staged: Option<PathBuf>,
    backup: Option<PathBuf>,
    installed: bool,
    diff: String,
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

        let head_tree = repository
            .head()
            .ok()
            .and_then(|head| head.peel_to_tree().ok());
        let diff = repository.diff_tree_to_workdir(head_tree.as_ref(), None)?;
        let output = std::cell::RefCell::new(Vec::new());
        let truncated = std::cell::Cell::new(false);
        diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
            let mut output = output.borrow_mut();
            let available = max_diff_bytes.saturating_sub(output.len());
            if available == 0 {
                truncated.set(true);
                return true;
            }
            let prefix = matches!(line.origin(), '+' | '-' | ' ').then_some(line.origin() as u8);
            let content = line.content();
            let total = content.len().saturating_add(usize::from(prefix.is_some()));
            if let Some(prefix) = prefix
                && output.len() < max_diff_bytes
            {
                output.push(prefix);
            }
            let remaining = max_diff_bytes.saturating_sub(output.len());
            output.extend_from_slice(&content[..content.len().min(remaining)]);
            truncated.set(truncated.get() || total > available);
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

    pub fn apply_structured_patch(
        &self,
        patch: &StructuredPatch,
        max_diff_bytes: usize,
    ) -> Result<PatchTransactionOutcome> {
        self.apply_structured_patch_inner(patch, max_diff_bytes, None)
    }

    fn apply_structured_patch_inner(
        &self,
        patch: &StructuredPatch,
        max_diff_bytes: usize,
        fail_after_commits: Option<usize>,
    ) -> Result<PatchTransactionOutcome> {
        const MAX_EDITS: usize = 100;
        const MAX_CONTENT_BYTES: usize = 8 * 1024 * 1024;
        if patch.edits.is_empty() || patch.edits.len() > MAX_EDITS {
            return Err(AxiomError::Tool(format!(
                "a patch must contain between 1 and {MAX_EDITS} edits"
            )));
        }
        let mut prepared = Vec::with_capacity(patch.edits.len());
        let mut paths = HashSet::new();
        let mut content_bytes = 0_usize;
        for edit in &patch.edits {
            let item = match self.prepare_edit(edit, &mut content_bytes, MAX_CONTENT_BYTES) {
                Ok(item) => item,
                Err(error) => {
                    cleanup_staged(&prepared);
                    return Err(error);
                }
            };
            if !paths.insert(item.path.clone()) {
                cleanup_staged(&prepared);
                if let Some(staged) = &item.staged {
                    let _ = std::fs::remove_file(staged);
                }
                return Err(AxiomError::Tool(format!(
                    "patch contains duplicate target {}",
                    item.relative.display()
                )));
            }
            prepared.push(item);
        }

        let commit = (|| -> Result<()> {
            for (index, edit) in prepared.iter_mut().enumerate() {
                match edit.operation {
                    PreparedOperation::Create => {
                        if std::fs::symlink_metadata(&edit.path).is_ok() {
                            return Err(AxiomError::Tool(format!(
                                "create conflict: {} now exists",
                                edit.relative.display()
                            )));
                        }
                        std::fs::rename(
                            edit.staged.as_ref().expect("create has staged file"),
                            &edit.path,
                        )?;
                        edit.staged = None;
                        edit.installed = true;
                    }
                    PreparedOperation::Update | PreparedOperation::Delete => {
                        let current = read_edit_target(&edit.path, &self.root, MAX_CONTENT_BYTES)?;
                        let current_sha = digest(&current.0);
                        if edit.expected_sha256.as_deref() != Some(&current_sha) {
                            return Err(AxiomError::Tool(format!(
                                "stale edit during commit for {}: expected {}, found {current_sha}",
                                edit.relative.display(),
                                edit.expected_sha256.as_deref().unwrap_or("(missing)")
                            )));
                        }
                        let backup = sibling_artifact(&edit.path, "backup");
                        std::fs::rename(&edit.path, &backup)?;
                        edit.backup = Some(backup);
                        if edit.operation == PreparedOperation::Update {
                            std::fs::rename(
                                edit.staged.as_ref().expect("update has staged file"),
                                &edit.path,
                            )?;
                            edit.staged = None;
                            edit.installed = true;
                        }
                    }
                }
                if fail_after_commits == Some(index + 1) {
                    return Err(AxiomError::Tool(format!(
                        "injected commit failure after {} edit(s)",
                        index + 1
                    )));
                }
            }
            Ok(())
        })();

        if let Err(error) = commit {
            let rollback_errors = rollback_prepared(&mut prepared);
            if rollback_errors.is_empty() {
                return Err(error);
            }
            return Err(AxiomError::Tool(format!(
                "{error}; rollback needs manual recovery: {}",
                rollback_errors.join("; ")
            )));
        }

        let mut warnings = Vec::new();
        for edit in &prepared {
            if let Some(backup) = &edit.backup
                && let Err(error) = std::fs::remove_file(backup)
            {
                warnings.push(format!(
                    "committed edit but could not remove backup {}: {error}",
                    backup.display()
                ));
            }
        }
        let mut diff = prepared
            .iter()
            .map(|edit| edit.diff.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let mut diff_truncated = false;
        diff = truncate_utf8(&diff, max_diff_bytes, &mut diff_truncated);
        let changed_paths = prepared
            .iter()
            .map(|edit| edit.relative.clone())
            .collect::<Vec<_>>();
        let edits = prepared
            .into_iter()
            .map(|edit| StructuredEditOutcome {
                operation: match edit.operation {
                    PreparedOperation::Create => "create",
                    PreparedOperation::Update => "update",
                    PreparedOperation::Delete => "delete",
                }
                .into(),
                path: edit.relative,
                old_sha256: edit.old_sha256,
                new_sha256: edit.new_sha256,
            })
            .collect();
        Ok(PatchTransactionOutcome {
            edits,
            changed_paths,
            diff,
            diff_truncated,
            warnings,
        })
    }

    fn prepare_edit(
        &self,
        edit: &EditIntent,
        total_content_bytes: &mut usize,
        max_content_bytes: usize,
    ) -> Result<PreparedEdit> {
        match edit {
            EditIntent::Create {
                path,
                content,
                mode,
            } => {
                if mode.is_some_and(|mode| mode > 0o777) {
                    return Err(AxiomError::Tool(
                        "create mode must be between 0000 and 0777".into(),
                    ));
                }
                *total_content_bytes = total_content_bytes.saturating_add(content.len());
                ensure_patch_content_budget(*total_content_bytes, max_content_bytes)?;
                let path = self.resolve_for_write(path)?;
                if std::fs::symlink_metadata(&path).is_ok() {
                    return Err(AxiomError::Tool(format!(
                        "create target already exists: {}",
                        self.relative(&path).display()
                    )));
                }
                let staged = stage_content(&path, content.as_bytes(), None, *mode)?;
                let relative = self.relative(&path);
                Ok(PreparedEdit {
                    operation: PreparedOperation::Create,
                    relative: relative.clone(),
                    path: path.clone(),
                    expected_sha256: None,
                    old_sha256: None,
                    new_sha256: Some(digest(content.as_bytes())),
                    staged: Some(staged),
                    backup: None,
                    installed: false,
                    diff: file_diff(&relative, None, Some(content)),
                })
            }
            EditIntent::Update {
                path,
                expected_sha256,
                content,
                preserve_line_endings,
            } => {
                let path = self.resolve_existing(path)?;
                let (old, permissions) = read_edit_target(&path, &self.root, max_content_bytes)?;
                let old_sha256 = digest(&old);
                if expected_sha256 != &old_sha256 {
                    return Err(AxiomError::Tool(format!(
                        "stale edit: expected {expected_sha256}, found {old_sha256}"
                    )));
                }
                let old_text = String::from_utf8(old).map_err(|_| {
                    AxiomError::Tool("structured updates require a UTF-8 target".into())
                })?;
                let content = if *preserve_line_endings {
                    preserve_newlines(&old_text, content)
                } else {
                    content.clone()
                };
                *total_content_bytes = total_content_bytes.saturating_add(content.len());
                ensure_patch_content_budget(*total_content_bytes, max_content_bytes)?;
                let staged = stage_content(&path, content.as_bytes(), Some(permissions), None)?;
                let relative = self.relative(&path);
                Ok(PreparedEdit {
                    operation: PreparedOperation::Update,
                    relative: relative.clone(),
                    path: path.clone(),
                    expected_sha256: Some(expected_sha256.clone()),
                    old_sha256: Some(old_sha256),
                    new_sha256: Some(digest(content.as_bytes())),
                    staged: Some(staged),
                    backup: None,
                    installed: false,
                    diff: file_diff(&relative, Some(&old_text), Some(&content)),
                })
            }
            EditIntent::Delete {
                path,
                expected_sha256,
            } => {
                let path = self.resolve_existing(path)?;
                let (old, _permissions) = read_edit_target(&path, &self.root, max_content_bytes)?;
                let old_sha256 = digest(&old);
                if expected_sha256 != &old_sha256 {
                    return Err(AxiomError::Tool(format!(
                        "stale delete: expected {expected_sha256}, found {old_sha256}"
                    )));
                }
                let old_text = String::from_utf8(old).map_err(|_| {
                    AxiomError::Tool("structured deletes require a UTF-8 target".into())
                })?;
                let relative = self.relative(&path);
                Ok(PreparedEdit {
                    operation: PreparedOperation::Delete,
                    relative: relative.clone(),
                    path: path.clone(),
                    expected_sha256: Some(expected_sha256.clone()),
                    old_sha256: Some(old_sha256),
                    new_sha256: None,
                    staged: None,
                    backup: None,
                    installed: false,
                    diff: file_diff(&relative, Some(&old_text), None),
                })
            }
        }
    }

    pub fn apply_replacement(
        &self,
        path: impl AsRef<Path>,
        old: &str,
        new: &str,
        expected_sha256: Option<&str>,
    ) -> Result<PatchOutcome> {
        let path = self.resolve_for_write(path)?;
        let (old_content, old_sha256) = if path.exists() {
            let (bytes, _permissions) = read_edit_target(&path, &self.root, 8 * 1024 * 1024)?;
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
                Some(sha),
            )
        } else {
            if !old.is_empty() {
                return Err(AxiomError::Tool(
                    "new file replacement must use an empty old string".into(),
                ));
            }
            (String::new(), None)
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

        let relative = self.relative(&path);
        let edit = if let Some(old_sha256) = &old_sha256 {
            EditIntent::Update {
                path: relative.clone(),
                expected_sha256: old_sha256.clone(),
                content: new_content.clone(),
                preserve_line_endings: false,
            }
        } else {
            EditIntent::Create {
                path: relative.clone(),
                content: new_content.clone(),
                mode: None,
            }
        };
        self.apply_structured_patch(&StructuredPatch { edits: vec![edit] }, 128 * 1024)?;

        Ok(PatchOutcome {
            path: relative,
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

fn ensure_patch_content_budget(bytes: usize, max_bytes: usize) -> Result<()> {
    if bytes > max_bytes {
        Err(AxiomError::Tool(format!(
            "patch content exceeds the {max_bytes}-byte transaction budget"
        )))
    } else {
        Ok(())
    }
}

fn read_edit_target(
    path: &Path,
    root: &Path,
    max_bytes: usize,
) -> Result<(Vec<u8>, std::fs::Permissions)> {
    let mut file = open_verified(path, root)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(AxiomError::Tool(format!(
            "edit target is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
        return Err(AxiomError::Tool(format!(
            "edit target exceeds the {max_bytes}-byte transaction budget: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.read_to_end(&mut bytes)?;
    Ok((bytes, metadata.permissions()))
}

fn stage_content(
    path: &Path,
    content: &[u8],
    permissions: Option<std::fs::Permissions>,
    mode: Option<u32>,
) -> Result<PathBuf> {
    let staged = sibling_artifact(path, "stage");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&staged)?;
        file.write_all(content)?;
        file.sync_all()?;
        if let Some(permissions) = permissions {
            std::fs::set_permissions(&staged, permissions)?;
        }
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        if mode.is_some() {
            return Err(AxiomError::Tool(
                "explicit file modes are not supported on this platform".into(),
            ));
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }
    Ok(staged)
}

fn sibling_artifact(path: &Path, purpose: &str) -> PathBuf {
    path.with_file_name(format!(
        ".{}.axiomcli-{purpose}-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("edit"),
        Uuid::new_v4()
    ))
}

fn cleanup_staged(prepared: &[PreparedEdit]) {
    for edit in prepared {
        if let Some(staged) = &edit.staged {
            let _ = std::fs::remove_file(staged);
        }
    }
}

fn rollback_prepared(prepared: &mut [PreparedEdit]) -> Vec<String> {
    let mut errors = Vec::new();
    for edit in prepared.iter_mut().rev() {
        if edit.installed
            && edit.path.exists()
            && let Err(error) = std::fs::remove_file(&edit.path)
        {
            errors.push(format!("remove {}: {error}", edit.path.display()));
            continue;
        }
        if let Some(backup) = &edit.backup
            && backup.exists()
            && let Err(error) = std::fs::rename(backup, &edit.path)
        {
            errors.push(format!(
                "restore {} from {}: {error}",
                edit.path.display(),
                backup.display()
            ));
        }
        if let Some(staged) = &edit.staged {
            let _ = std::fs::remove_file(staged);
        }
    }
    errors
}

fn preserve_newlines(old: &str, new: &str) -> String {
    let old_crlf = old
        .as_bytes()
        .windows(2)
        .filter(|pair| *pair == b"\r\n")
        .count();
    let old_lf = old.split('\n').count().saturating_sub(1);
    if old_crlf > 0 && old_crlf == old_lf {
        new.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        new.to_owned()
    }
}

fn file_diff(path: &Path, old: Option<&str>, new: Option<&str>) -> String {
    let display = path.to_string_lossy();
    let old_label = if old.is_some() {
        format!("a/{display}")
    } else {
        "/dev/null".into()
    };
    let new_label = if new.is_some() {
        format!("b/{display}")
    } else {
        "/dev/null".into()
    };
    format!(
        "--- {old_label}\n+++ {new_label}\n{}",
        unified_diff(old.unwrap_or_default(), new.unwrap_or_default())
    )
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
    #[serde(default)]
    pub cwd: PathBuf,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
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
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
    pub outcome: Option<ProcessOutcome>,
}

#[derive(Clone)]
struct BackgroundTask {
    cancellation: CancellationToken,
    state: Arc<Mutex<BackgroundSnapshot>>,
    completion: watch::Receiver<String>,
}

#[derive(Clone)]
pub struct ProcessManager {
    tasks: Arc<Mutex<HashMap<String, BackgroundTask>>>,
    max_output_bytes: usize,
    max_background_tasks: usize,
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new(64 * 1024)
    }
}

impl ProcessManager {
    #[must_use]
    pub fn new(max_output_bytes: usize) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            max_output_bytes: max_output_bytes.max(1024),
            max_background_tasks: 32,
        }
    }

    pub async fn run(
        &self,
        workspace: &Workspace,
        request: &ProcessRequest,
        cancellation: CancellationToken,
    ) -> Result<ProcessOutcome> {
        self.run_inner(workspace, request, cancellation, None).await
    }

    async fn run_inner(
        &self,
        workspace: &Workspace,
        request: &ProcessRequest,
        cancellation: CancellationToken,
        live: Option<Arc<Mutex<BackgroundSnapshot>>>,
    ) -> Result<ProcessOutcome> {
        if request.program.trim().is_empty() {
            return Err(AxiomError::Tool("program cannot be empty".into()));
        }
        if request.program.contains('\0')
            || request.args.iter().any(|argument| argument.contains('\0'))
        {
            return Err(AxiomError::Tool(
                "program and arguments must not contain NUL bytes".into(),
            ));
        }
        for (key, value) in &request.env {
            if !crate::policy::is_allowed_child_env_key(key) || value.contains('\0') {
                return Err(AxiomError::Tool(format!(
                    "child environment variable `{key}` is not allowlisted"
                )));
            }
        }
        let cwd = if request.cwd.as_os_str().is_empty() {
            workspace.root().to_path_buf()
        } else {
            workspace.resolve_existing(&request.cwd)?
        };
        if !cwd.is_dir() {
            return Err(AxiomError::Tool(format!(
                "process cwd is not a directory: {}",
                cwd.display()
            )));
        }
        let mut command = sandboxed_command(workspace, &cwd, request)?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
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
        let retained = Arc::new(AtomicUsize::new(0));
        let max = self.max_output_bytes;
        let stdout_task = tokio::spawn(read_bounded(
            stdout,
            max,
            retained.clone(),
            live.clone(),
            OutputStream::Stdout,
        ));
        let stderr_task = tokio::spawn(read_bounded(
            stderr,
            max,
            retained,
            live,
            OutputStream::Stderr,
        ));
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
        let (mut stdout, stdout_truncated) = stdout_task
            .await
            .map_err(|error| AxiomError::Tool(error.to_string()))??;
        let (mut stderr, stderr_truncated) = stderr_task
            .await
            .map_err(|error| AxiomError::Tool(error.to_string()))??;
        let mut encoding_truncated = false;
        stdout = truncate_utf8(&stdout, self.max_output_bytes, &mut encoding_truncated);
        let remaining = self.max_output_bytes.saturating_sub(stdout.len());
        stderr = truncate_utf8(&stderr, remaining, &mut encoding_truncated);
        Ok(ProcessOutcome {
            exit_code: status.code(),
            stdout,
            stderr,
            truncated: stdout_truncated || stderr_truncated || encoding_truncated,
        })
    }

    pub async fn start(&self, workspace: Workspace, request: ProcessRequest) -> Result<String> {
        if self.tasks.lock().await.len() >= self.max_background_tasks {
            return Err(AxiomError::Tool(format!(
                "background task limit reached ({})",
                self.max_background_tasks
            )));
        }
        let id = Uuid::new_v4().to_string();
        let cancellation = CancellationToken::new();
        let state = Arc::new(Mutex::new(BackgroundSnapshot {
            id: id.clone(),
            state: "running".into(),
            stdout: String::new(),
            stderr: String::new(),
            truncated: false,
            outcome: None,
        }));
        let (completion_tx, completion) = watch::channel("running".to_owned());
        self.tasks.lock().await.insert(
            id.clone(),
            BackgroundTask {
                cancellation: cancellation.clone(),
                state: state.clone(),
                completion,
            },
        );
        let manager = self.clone();
        tokio::spawn(async move {
            let outcome = manager
                .run_inner(&workspace, &request, cancellation, Some(state.clone()))
                .await;
            let mut snapshot = state.lock().await;
            match outcome {
                Ok(outcome) => {
                    snapshot.state = "completed".into();
                    snapshot.stdout.clone_from(&outcome.stdout);
                    snapshot.stderr.clone_from(&outcome.stderr);
                    snapshot.truncated = outcome.truncated;
                    snapshot.outcome = Some(outcome);
                }
                Err(AxiomError::Cancelled) => snapshot.state = "cancelled".into(),
                Err(error) => {
                    snapshot.state = format!("failed: {error}");
                }
            }
            let _ = completion_tx.send(snapshot.state.clone());
        });
        Ok(id)
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
        drop(task);
        let _ = self.wait(id, 10).await?;
        Ok(())
    }

    pub async fn list(&self) -> Vec<BackgroundSnapshot> {
        let states = self
            .tasks
            .lock()
            .await
            .values()
            .map(|task| task.state.clone())
            .collect::<Vec<_>>();
        let mut snapshots = Vec::with_capacity(states.len());
        for state in states {
            snapshots.push(state.lock().await.clone());
        }
        snapshots.sort_by(|left, right| left.id.cmp(&right.id));
        snapshots
    }

    pub async fn wait(&self, id: &str, timeout_secs: u64) -> Result<BackgroundSnapshot> {
        let mut completion = self
            .tasks
            .lock()
            .await
            .get(id)
            .map(|task| task.completion.clone())
            .ok_or_else(|| AxiomError::Tool(format!("unknown background task `{id}`")))?;
        if completion.borrow().as_str() == "running" {
            tokio::time::timeout(
                Duration::from_secs(timeout_secs.clamp(1, 3600)),
                completion.changed(),
            )
            .await
            .map_err(|_| {
                AxiomError::Tool(format!(
                    "background wait timed out after {timeout_secs} seconds"
                ))
            })?
            .map_err(|_| AxiomError::Tool("background task watcher closed".into()))?;
        }
        self.snapshot(id).await
    }
}

#[derive(Clone, Copy)]
enum OutputStream {
    Stdout,
    Stderr,
}

async fn read_bounded(
    mut reader: impl AsyncRead + Unpin,
    max_bytes: usize,
    retained_bytes: Arc<AtomicUsize>,
    live: Option<Arc<Mutex<BackgroundSnapshot>>>,
    stream: OutputStream,
) -> Result<(String, bool)> {
    let mut retained = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let accepted = reserve_output(&retained_bytes, max_bytes, read);
        retained.extend_from_slice(&buffer[..accepted]);
        truncated |= accepted < read;
        if let Some(live) = &live {
            let chunk = sanitize_terminal(&String::from_utf8_lossy(&buffer[..accepted]));
            let mut snapshot = live.lock().await;
            match stream {
                OutputStream::Stdout => snapshot.stdout.push_str(&chunk),
                OutputStream::Stderr => snapshot.stderr.push_str(&chunk),
            }
            snapshot.truncated |= accepted < read;
        }
    }
    let text = sanitize_terminal(&String::from_utf8_lossy(&retained));
    Ok((text, truncated))
}

fn reserve_output(retained: &AtomicUsize, maximum: usize, requested: usize) -> usize {
    let mut current = retained.load(Ordering::Relaxed);
    loop {
        let accepted = requested.min(maximum.saturating_sub(current));
        match retained.compare_exchange_weak(
            current,
            current.saturating_add(accepted),
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => return accepted,
            Err(actual) => current = actual,
        }
    }
}

fn sanitize_terminal(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' {
            match characters.peek().copied() {
                Some('[') => {
                    characters.next();
                    for next in characters.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    characters.next();
                    while let Some(next) = characters.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' && characters.peek() == Some(&'\\') {
                            characters.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    characters.next();
                }
                None => {}
            }
        } else if character == '\n' || character == '\t' || !character.is_control() {
            output.push(character);
        }
    }
    output
}

fn sandboxed_command(
    workspace: &Workspace,
    cwd: &Path,
    request: &ProcessRequest,
) -> Result<Command> {
    #[cfg(target_os = "linux")]
    {
        let bwrap = Path::new("/usr/bin/bwrap");
        if !bwrap.is_file() {
            return Err(AxiomError::Tool(
                "process isolation requires /usr/bin/bwrap on Linux".into(),
            ));
        }
        let mut command = Command::new(bwrap);
        command.env_clear().args([
            "--die-with-parent",
            "--unshare-all",
            "--clearenv",
            "--ro-bind",
            "/usr",
            "/usr",
            "--ro-bind",
            "/etc",
            "/etc",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--dir",
            "/tmp/axiom-home",
            "--dir",
            "/tmp/cargo-home",
            "--dir",
            "/opt",
            "--dir",
            "/opt/axiomcli",
        ]);
        for system_path in ["/bin", "/lib", "/lib64", "/sbin"] {
            if Path::new(system_path).exists() {
                command.args(["--ro-bind", system_path, system_path]);
            }
        }
        command.args([
            "--bind",
            &workspace.root().to_string_lossy(),
            &workspace.root().to_string_lossy(),
            "--chdir",
            &cwd.to_string_lossy(),
            "--setenv",
            "HOME",
            "/tmp/axiom-home",
            "--setenv",
            "CARGO_HOME",
            "/tmp/cargo-home",
            "--setenv",
            "PATH",
            "/opt/axiomcli/cargo-bin:/usr/bin:/bin",
            "--setenv",
            "LANG",
            "C.UTF-8",
        ]);
        bind_development_cache(&mut command);
        for (key, value) in &request.env {
            command.args(["--setenv", key, value]);
        }
        command.arg("--").arg(&request.program).args(&request.args);
        Ok(command)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let mut command = Command::new(&request.program);
        command
            .args(&request.args)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LANG", "C.UTF-8");
        for (key, value) in &request.env {
            command.env(key, value);
        }
        Ok(command)
    }
}

#[cfg(target_os = "linux")]
fn bind_development_cache(command: &mut Command) {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let cargo = home.join(".cargo");
    let mounts = [
        (cargo.join("bin"), PathBuf::from("/opt/axiomcli/cargo-bin")),
        (home.join(".rustup"), PathBuf::from("/opt/axiomcli/rustup")),
        (
            cargo.join("registry"),
            PathBuf::from("/tmp/cargo-home/registry"),
        ),
        (cargo.join("git"), PathBuf::from("/tmp/cargo-home/git")),
    ];
    for (source, target) in mounts {
        if source.exists() {
            command.arg("--ro-bind").arg(source).arg(target);
        }
    }
    if home.join(".rustup").exists() {
        command.args(["--setenv", "RUSTUP_HOME", "/opt/axiomcli/rustup"]);
    }
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

    #[test]
    fn structured_patch_creates_updates_deletes_and_preserves_newlines_and_mode() {
        let root = tempdir().expect("root");
        std::fs::write(root.path().join("update.txt"), "old\r\nline\r\n").expect("update");
        std::fs::write(root.path().join("delete.txt"), "remove me\n").expect("delete");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(
                root.path().join("update.txt"),
                std::fs::Permissions::from_mode(0o744),
            )
            .expect("mode");
        }
        let workspace = Workspace::new(root.path()).expect("workspace");
        let update_hash = workspace
            .read("update.txt", 1, 20, 1024)
            .expect("read update")
            .sha256;
        let delete_hash = workspace
            .read("delete.txt", 1, 20, 1024)
            .expect("read delete")
            .sha256;
        let patch = StructuredPatch {
            edits: vec![
                EditIntent::Update {
                    path: PathBuf::from("update.txt"),
                    expected_sha256: update_hash,
                    content: "new\n🌸\n".into(),
                    preserve_line_endings: true,
                },
                EditIntent::Create {
                    path: PathBuf::from("created.txt"),
                    content: "created λ\n".into(),
                    mode: Some(0o640),
                },
                EditIntent::Delete {
                    path: PathBuf::from("delete.txt"),
                    expected_sha256: delete_hash,
                },
            ],
        };
        let outcome = workspace
            .apply_structured_patch(&patch, 16 * 1024)
            .expect("transaction");
        assert_eq!(outcome.edits.len(), 3);
        assert_eq!(
            std::fs::read(root.path().join("update.txt")).expect("updated"),
            "new\r\n🌸\r\n".as_bytes()
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("created.txt")).expect("created"),
            "created λ\n"
        );
        assert!(!root.path().join("delete.txt").exists());
        assert!(outcome.diff.contains("--- a/update.txt"));
        assert!(outcome.diff.contains("+++ /dev/null"));
        assert!(outcome.diff.contains("+++ b/created.txt"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(root.path().join("update.txt"))
                    .expect("updated metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o744
            );
            assert_eq!(
                std::fs::metadata(root.path().join("created.txt"))
                    .expect("created metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o640
            );
        }
        let created_hash = workspace
            .read("created.txt", 1, 20, 1024)
            .expect("created hash")
            .sha256;
        let truncated = workspace
            .apply_structured_patch(
                &StructuredPatch {
                    edits: vec![EditIntent::Update {
                        path: PathBuf::from("created.txt"),
                        expected_sha256: created_hash,
                        content: "a substantially longer replacement\n".into(),
                        preserve_line_endings: true,
                    }],
                },
                8,
            )
            .expect("truncated preview");
        assert!(truncated.diff_truncated);
        assert!(truncated.diff.len() <= 8);
    }

    #[test]
    fn structured_patch_prevalidation_conflict_and_partial_commit_roll_back() {
        let root = tempdir().expect("root");
        std::fs::write(root.path().join("first.txt"), "first old\n").expect("first");
        std::fs::write(root.path().join("second.txt"), "second old\n").expect("second");
        let workspace = Workspace::new(root.path()).expect("workspace");
        let first_hash = workspace
            .read("first.txt", 1, 20, 1024)
            .expect("first read")
            .sha256;
        let conflict = StructuredPatch {
            edits: vec![
                EditIntent::Update {
                    path: PathBuf::from("first.txt"),
                    expected_sha256: first_hash.clone(),
                    content: "first new\n".into(),
                    preserve_line_endings: true,
                },
                EditIntent::Update {
                    path: PathBuf::from("second.txt"),
                    expected_sha256: "0".repeat(64),
                    content: "second new\n".into(),
                    preserve_line_endings: true,
                },
            ],
        };
        assert!(workspace.apply_structured_patch(&conflict, 4096).is_err());
        assert_eq!(
            std::fs::read_to_string(root.path().join("first.txt")).expect("unchanged"),
            "first old\n"
        );

        let rollback = StructuredPatch {
            edits: vec![
                EditIntent::Update {
                    path: PathBuf::from("first.txt"),
                    expected_sha256: first_hash,
                    content: "first new\n".into(),
                    preserve_line_endings: true,
                },
                EditIntent::Create {
                    path: PathBuf::from("created.txt"),
                    content: "created\n".into(),
                    mode: None,
                },
            ],
        };
        let error = workspace
            .apply_structured_patch_inner(&rollback, 4096, Some(1))
            .expect_err("injected failure");
        assert!(error.to_string().contains("injected commit failure"));
        assert_eq!(
            std::fs::read_to_string(root.path().join("first.txt")).expect("rolled back"),
            "first old\n"
        );
        assert!(!root.path().join("created.txt").exists());
        assert!(
            std::fs::read_dir(root.path())
                .expect("entries")
                .all(|entry| !entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .contains(".axiomcli-"))
        );
    }

    #[test]
    fn structured_patch_rejects_escape_duplicate_unknown_and_symlink_targets() {
        let root = tempdir().expect("root");
        let outside = tempdir().expect("outside");
        let workspace = Workspace::new(root.path()).expect("workspace");
        let escape = StructuredPatch {
            edits: vec![EditIntent::Create {
                path: PathBuf::from("../escape.txt"),
                content: "no".into(),
                mode: None,
            }],
        };
        assert!(workspace.apply_structured_patch(&escape, 4096).is_err());
        assert!(!outside.path().join("escape.txt").exists());
        let duplicate = StructuredPatch {
            edits: vec![
                EditIntent::Create {
                    path: PathBuf::from("same.txt"),
                    content: "one".into(),
                    mode: None,
                },
                EditIntent::Create {
                    path: PathBuf::from("same.txt"),
                    content: "two".into(),
                    mode: None,
                },
            ],
        };
        assert!(workspace.apply_structured_patch(&duplicate, 4096).is_err());
        assert!(!root.path().join("same.txt").exists());
        assert!(
            serde_json::from_value::<StructuredPatch>(serde_json::json!({
                "edits": [{"operation":"create", "path":"x", "content":"x", "surprise":true}]
            }))
            .is_err()
        );

        #[cfg(unix)]
        {
            std::fs::write(outside.path().join("secret"), "outside").expect("outside file");
            std::os::unix::fs::symlink(outside.path().join("secret"), root.path().join("link"))
                .expect("symlink");
            let patch = StructuredPatch {
                edits: vec![EditIntent::Delete {
                    path: PathBuf::from("link"),
                    expected_sha256: digest(b"outside"),
                }],
            };
            assert!(workspace.apply_structured_patch(&patch, 4096).is_err());
            assert_eq!(
                std::fs::read_to_string(outside.path().join("secret")).expect("outside intact"),
                "outside"
            );
        }
    }

    #[test]
    fn structured_diff_and_git_diff_agree_on_changed_lines() {
        let root = tempdir().expect("root");
        let repository = Repository::init(root.path()).expect("repository");
        std::fs::write(root.path().join("file.txt"), "old\n").expect("file");
        let mut index = repository.index().expect("index");
        index.add_path(Path::new("file.txt")).expect("add");
        let tree_id = index.write_tree().expect("tree");
        let tree = repository.find_tree(tree_id).expect("tree");
        let signature = Signature::now("AxiomCLI", "test@example.invalid").expect("signature");
        repository
            .commit(Some("HEAD"), &signature, &signature, "base", &tree, &[])
            .expect("commit");
        drop(tree);
        let workspace = Workspace::new(root.path()).expect("workspace");
        let hash = workspace.read("file.txt", 1, 10, 100).expect("read").sha256;
        let outcome = workspace
            .apply_structured_patch(
                &StructuredPatch {
                    edits: vec![EditIntent::Update {
                        path: PathBuf::from("file.txt"),
                        expected_sha256: hash,
                        content: "new content\n".into(),
                        preserve_line_endings: true,
                    }],
                },
                4096,
            )
            .expect("patch");
        let git = workspace.inspect_git(10, 1, 4096).expect("git diff");
        for line in ["-old", "+new content"] {
            assert!(outcome.diff.contains(line), "structured {line}");
            assert!(git.diff.contains(line), "git {line}: {:?}", git.diff);
        }
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
                    cwd: PathBuf::new(),
                    env: BTreeMap::new(),
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
                    cwd: PathBuf::new(),
                    env: BTreeMap::new(),
                    timeout_secs: 5,
                },
                CancellationToken::new(),
            )
            .await
            .expect("escaped process");
        assert!(!escaped.stdout.contains('\u{1b}'));
        assert!(!escaped.stdout.contains("[31m"));
    }

    fn process_request(program: &str, args: &[&str]) -> ProcessRequest {
        ProcessRequest {
            program: program.into(),
            args: args.iter().map(|argument| (*argument).into()).collect(),
            cwd: PathBuf::new(),
            env: BTreeMap::new(),
            timeout_secs: 5,
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn process_isolated_from_network_host_files_and_credentials() {
        let root = tempdir().expect("root");
        let workspace = Workspace::new(root.path()).expect("workspace");
        let manager = ProcessManager::new(4096);

        let host_read = manager
            .run(
                &workspace,
                &process_request("sh", &["-c", "test ! -e /home && test ! -e /root"]),
                CancellationToken::new(),
            )
            .await
            .expect("host boundary");
        assert_eq!(host_read.exit_code, Some(0));

        // Credential-shaped variables are absent from the deliberately tiny
        // child environment, even when callers have them in their environment.
        let environment = manager
            .run(
                &workspace,
                &process_request("sh", &["-c", "printf '%s' \"${AXIOM_API_KEY-unset}\""]),
                CancellationToken::new(),
            )
            .await
            .expect("environment");
        assert_eq!(environment.stdout, "unset");

        let network = manager
            .run(
                &workspace,
                &process_request(
                    "python3",
                    &[
                        "-c",
                        "import socket; socket.socket().connect(('1.1.1.1', 53))",
                    ],
                ),
                CancellationToken::new(),
            )
            .await
            .expect("network attempt returns an outcome");
        assert_ne!(network.exit_code, Some(0));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn process_cwd_and_environment_are_explicit_and_allowlisted() {
        let root = tempdir().expect("root");
        std::fs::create_dir(root.path().join("nested")).expect("nested");
        let workspace = Workspace::new(root.path()).expect("workspace");
        let manager = ProcessManager::new(4096);
        let mut request = process_request("sh", &["-c", "printf '%s:%s' \"$PWD\" \"$CI\""]);
        request.cwd = PathBuf::from("nested");
        request.env.insert("CI".into(), "true".into());
        let outcome = manager
            .run(&workspace, &request, CancellationToken::new())
            .await
            .expect("process");
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(
            outcome.stdout,
            format!("{}:true", root.path().join("nested").display())
        );

        request.env.insert("AXIOM_API_KEY".into(), "blocked".into());
        assert!(
            manager
                .run(&workspace, &request, CancellationToken::new())
                .await
                .is_err()
        );
        request.env.clear();
        request.cwd = PathBuf::from("..");
        assert!(
            manager
                .run(&workspace, &request, CancellationToken::new())
                .await
                .is_err()
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancellation_terminates_the_process_tree() {
        let root = tempdir().expect("root");
        let workspace = Workspace::new(root.path()).expect("workspace");
        let manager = ProcessManager::new(4096);
        let cancellation = CancellationToken::new();
        let cancel = cancellation.clone();
        let request = process_request(
            "sh",
            &["-c", "(sleep 1; printf survived > child-survived) & wait"],
        );
        let run = manager.run(&workspace, &request, cancellation);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        });
        assert!(matches!(run.await, Err(AxiomError::Cancelled)));
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert!(!root.path().join("child-survived").exists());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn timeout_invalid_bytes_and_background_lifecycle_are_recoverable() {
        let root = tempdir().expect("root");
        let workspace = Workspace::new(root.path()).expect("workspace");
        let manager = ProcessManager::new(1024);

        let mut timeout = process_request("sleep", &["5"]);
        timeout.timeout_secs = 1;
        assert!(
            manager
                .run(&workspace, &timeout, CancellationToken::new())
                .await
                .is_err()
        );

        let invalid = manager
            .run(
                &workspace,
                &process_request(
                    "python3",
                    &["-c", "import os; os.write(1, b'\\xff\\xfe\\x1b[31mred')"],
                ),
                CancellationToken::new(),
            )
            .await
            .expect("invalid bytes");
        assert!(!invalid.stdout.contains('\u{1b}'));
        assert!(!invalid.stdout.contains("[31m"));
        assert!(invalid.stdout.len() <= 1024);

        let id = manager
            .start(
                workspace.clone(),
                process_request("sh", &["-c", "printf start; sleep 5"]),
            )
            .await
            .expect("start");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let running = manager.snapshot(&id).await.expect("snapshot");
        assert_eq!(running.state, "running");
        assert!(running.stdout.contains("start"));
        assert!(manager.list().await.iter().any(|task| task.id == id));
        manager.stop(&id).await.expect("stop");
        let stopped = manager.snapshot(&id).await.expect("stopped snapshot");
        assert_eq!(stopped.state, "cancelled");
    }
}
