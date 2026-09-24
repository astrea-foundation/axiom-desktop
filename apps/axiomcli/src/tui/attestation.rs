//! Provider-neutral verification presentation and report exports.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Component, Path, PathBuf},
};

use axiom_secure_client::{SecurityEvidence, SecurityState};

use crate::{AxiomError, Result};

use super::text::sanitize_terminal_text;

pub(super) const MAX_WORKLOAD_DISPLAY_CHARS: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReportStatus {
    Idle,
    Unverified,
    Verifying,
    Verified,
    Degraded,
    Outdated,
    Failed,
    Expired,
    Unavailable,
}

impl ReportStatus {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Unverified => "NOT VERIFIED",
            Self::Verifying => "VERIFYING",
            Self::Verified => "TEE VERIFIED",
            Self::Degraded => "TEE UPDATES NEEDED",
            Self::Outdated => "TEE UPDATES REQUIRED",
            Self::Failed => "SECURITY FAILED",
            Self::Expired => "PROOF EXPIRED",
            Self::Unavailable => "PROOF UNAVAILABLE",
        }
    }
}

pub(super) fn report_status(
    security: &str,
    model: &str,
    evidence: Option<&SecurityEvidence>,
    now: u64,
) -> ReportStatus {
    match security {
        "VERIFYING" => return ReportStatus::Verifying,
        "SECURITY FAILED" => return ReportStatus::Failed,
        "TEE OUTDATED" => return ReportStatus::Outdated,
        "SECURE" | "TEE WARNING" => {}
        _ => return ReportStatus::Unverified,
    }
    let Some(evidence) = evidence.filter(|evidence| evidence.model_id == model) else {
        return ReportStatus::Unavailable;
    };
    match evidence.state {
        SecurityState::Expired => return ReportStatus::Expired,
        SecurityState::Rejected => return ReportStatus::Failed,
        SecurityState::Verified | SecurityState::Degraded => {}
        _ => return ReportStatus::Unavailable,
    }
    let degraded = evidence.state == SecurityState::Degraded && evidence.provider_id == "near";
    if evidence.checks.iter().any(|check| {
        !(check.passed || degraded && check.id == "intel_tdx" && check.status == "OutOfDate")
    }) {
        return ReportStatus::Failed;
    }
    let Some(expires) = evidence.hard_expires_at_unix_seconds else {
        return ReportStatus::Unavailable;
    };
    if evidence.verified_at_unix_seconds > now || expires <= evidence.verified_at_unix_seconds {
        return ReportStatus::Unavailable;
    }
    if expires <= now {
        ReportStatus::Expired
    } else if degraded {
        ReportStatus::Degraded
    } else {
        ReportStatus::Verified
    }
}

pub(super) fn attestation_export_filename(evidence: &SecurityEvidence) -> String {
    i64::try_from(evidence.verified_at_unix_seconds)
        .ok()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map_or_else(
            || {
                format!(
                    "axiom-attestation-{}.json",
                    evidence.verified_at_unix_seconds
                )
            },
            |time| format!("axiom-attestation-{}.json", time.format("%Y%m%d-%H%M%S")),
        )
}

pub(super) fn attestation_picker_directories(directory: &Path) -> Vec<(String, PathBuf)> {
    let mut directories = Vec::new();
    if let Some(parent) = directory.parent() {
        directories.push(("../".into(), parent.to_path_buf()));
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return directories;
    };
    let mut children = entries
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            path.is_dir().then(|| {
                let name = sanitize_terminal_text(&entry.file_name().to_string_lossy());
                (format!("{name}/"), path)
            })
        })
        .collect::<Vec<_>>();
    children.sort_by_key(|(name, _)| name.to_lowercase());
    directories.extend(children);
    directories
}

pub(super) fn write_attestation_export(
    directory: &Path,
    filename: &str,
    evidence: &SecurityEvidence,
) -> Result<PathBuf> {
    let mut components = Path::new(filename).components();
    let filename_is_safe =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    if !filename_is_safe {
        return Err(AxiomError::Config(
            "enter a filename without directory separators".into(),
        ));
    }

    let destination = directory.join(filename);
    let mut encoded = serde_json::to_vec_pretty(evidence)?;
    encoded.push(b'\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                AxiomError::Config(format!(
                    "{} already exists; choose another filename",
                    destination.display()
                ))
            } else {
                AxiomError::Io(error)
            }
        })?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    Ok(destination)
}

pub(super) fn humanize_evidence_key(key: &str) -> String {
    let mut words = key.split('_').filter(|word| !word.is_empty());
    let Some(first) = words.next() else {
        return key.to_owned();
    };
    let mut output = first.to_owned();
    if let Some(initial) = output.get_mut(0..1) {
        initial.make_ascii_uppercase();
    }
    for word in words {
        output.push(' ');
        output.push_str(word);
    }
    output
}

pub(super) fn bounded_workload_display(text: &str) -> (String, bool) {
    if text.chars().count() <= MAX_WORKLOAD_DISPLAY_CHARS {
        return (text.to_owned(), false);
    }
    (
        text.chars().take(MAX_WORKLOAD_DISPLAY_CHARS).collect(),
        true,
    )
}
