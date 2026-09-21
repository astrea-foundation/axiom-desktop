//! Attestation exports and workload presentation data.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Component, Path, PathBuf},
};

use axiom_secure_client::SecurityEvidence;

use crate::{AxiomError, Result};

use super::text::{sanitize_terminal_text, truncate_end};

pub(super) const MAX_WORKLOAD_DISPLAY_CHARS: usize = 100_000;

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

#[derive(Debug, Default)]
pub(super) struct ParsedWorkloadManifest {
    pub(super) metadata: Vec<(String, String)>,
    pub(super) compose: Option<String>,
    pub(super) services: Vec<WorkloadService>,
}

#[derive(Debug)]
pub(super) struct WorkloadService {
    pub(super) name: String,
    pub(super) image: Option<String>,
}

pub(super) fn parse_workload_manifest(manifest: &str) -> ParsedWorkloadManifest {
    let Ok(serde_json::Value::Object(object)) = serde_json::from_str(manifest) else {
        return ParsedWorkloadManifest {
            services: parse_compose_services(manifest),
            ..ParsedWorkloadManifest::default()
        };
    };
    let mut parsed = ParsedWorkloadManifest::default();
    for (key, value) in object {
        if key == "docker_compose_file" {
            if let Some(compose) = value.as_str() {
                parsed.services = parse_compose_services(compose);
                parsed.compose = Some(compose.to_owned());
            }
            continue;
        }
        let value = match value {
            serde_json::Value::Null => "null".to_owned(),
            serde_json::Value::String(value) => value,
            serde_json::Value::Bool(value) => value.to_string(),
            serde_json::Value::Number(value) => value.to_string(),
            value => serde_json::to_string(&value).unwrap_or_else(|_| "unavailable".into()),
        };
        parsed.metadata.push((key, truncate_end(&value, 160)));
    }
    parsed
}

pub(super) fn parse_compose_services(compose: &str) -> Vec<WorkloadService> {
    let mut services = Vec::new();
    let mut in_services = false;
    for line in compose.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len().saturating_sub(line.trim_start().len());
        if indent == 0 {
            in_services = trimmed == "services:";
            continue;
        }
        if !in_services {
            continue;
        }
        if indent == 2 && trimmed.ends_with(':') {
            if services.len() >= 128 {
                break;
            }
            services.push(WorkloadService {
                name: trimmed.trim_end_matches(':').to_owned(),
                image: None,
            });
        } else if indent == 4
            && let Some(image) = trimmed.strip_prefix("image:")
            && let Some(service) = services.last_mut()
        {
            service.image = Some(image.trim().trim_matches(['\'', '"']).to_owned());
        }
    }
    services
}

pub(super) fn humanize_manifest_key(key: &str) -> String {
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
