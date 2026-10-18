//! Storage value validation, redaction and platform path encoding.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::{
    AxiomError, Result,
    app::{AppEvent, EventEnvelope, Origin, PermissionProfile, ThinkingLevel},
    audit::redact_value,
};

use super::MAX_CLIENT_ITEM_ID_BYTES;

pub(super) fn origin_name(origin: Origin) -> &'static str {
    match origin {
        Origin::Tui => "tui",
        Origin::Acp => "acp",
        Origin::Headless => "headless",
        Origin::Test => "test",
    }
}

pub(super) fn origin_from_name(value: &str) -> Result<Origin> {
    match value {
        "tui" => Ok(Origin::Tui),
        "acp" => Ok(Origin::Acp),
        "headless" => Ok(Origin::Headless),
        "test" => Ok(Origin::Test),
        _ => Err(AxiomError::Storage(format!(
            "invalid origin `{value}` in local state"
        ))),
    }
}

pub(super) fn validate_model(model: &str) -> Result<&str> {
    let model = model.trim();
    if model.is_empty() || model.len() > 256 || model.chars().any(char::is_whitespace) {
        return Err(AxiomError::Storage(
            "model must be a non-empty model ID without whitespace".into(),
        ));
    }
    Ok(model)
}

pub(super) fn validate_client_item_id(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_CLIENT_ITEM_ID_BYTES
        || value.chars().any(char::is_whitespace)
    {
        return Err(AxiomError::Storage(format!(
            "client item ID must contain 1 to {MAX_CLIENT_ITEM_ID_BYTES} non-whitespace bytes"
        )));
    }
    Ok(value.into())
}

pub(super) fn redact_text(value: &str) -> String {
    let mut value = Value::String(value.into());
    redact_value(&mut value);
    value.as_str().unwrap_or_default().to_owned()
}

pub(super) fn redacted_json(value: &impl Serialize) -> Result<Value> {
    let mut value = serde_json::to_value(value)?;
    redact_value(&mut value);
    Ok(value)
}

pub(super) fn encode_metadata(mut metadata: Value) -> Result<String> {
    redact_value(&mut metadata);
    serde_json::to_string(&metadata).map_err(Into::into)
}

pub(super) fn checked_i64(value: u64, label: &str) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| AxiomError::Storage(format!("{label} exceeds SQLite integer range")))
}

pub(super) fn checked_u64(value: i64, label: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| AxiomError::Storage(format!("{label} is negative")))
}

pub(super) fn checked_u64_sql(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

pub(super) fn parse_time(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .map_or_else(|_| Utc::now(), |value| value.with_timezone(&Utc))
}

/// Return the model active for the latest submitted prompt. This projection
/// keeps the runtime restoration API independent from the `SQLite` schema.
#[must_use]
pub fn model_for_resume(events: &[EventEnvelope], fallback: &str) -> String {
    let mut active = fallback.to_owned();
    let mut last_used = None;
    for envelope in events {
        match &envelope.event {
            AppEvent::ModelChanged { model } => active.clone_from(model),
            AppEvent::PromptAccepted { .. } => last_used = Some(active.clone()),
            _ => {}
        }
    }
    last_used.unwrap_or(active)
}

#[must_use]
pub fn thinking_for_resume(events: &[EventEnvelope], fallback: ThinkingLevel) -> ThinkingLevel {
    events.iter().fold(fallback, |level, envelope| {
        if let AppEvent::ThinkingLevelChanged { level } = &envelope.event {
            *level
        } else {
            level
        }
    })
}

#[must_use]
pub fn permission_for_resume(
    events: &[EventEnvelope],
    fallback: PermissionProfile,
) -> PermissionProfile {
    events
        .iter()
        .fold(fallback, |profile, envelope| match &envelope.event {
            AppEvent::SessionCreated { profile, .. }
            | AppEvent::SessionResumed { profile, .. }
            | AppEvent::PermissionProfileChanged { profile } => *profile,
            _ => profile,
        })
}

#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the fallback platform implementation can reject non-Unicode paths"
)]
pub(super) fn encode_path(path: &Path) -> Result<(String, Vec<u8>)> {
    use std::os::unix::ffi::OsStrExt as _;
    Ok(("unix_bytes".into(), path.as_os_str().as_bytes().to_vec()))
}

#[cfg(unix)]
pub(super) fn decode_path(encoding: &str, bytes: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt as _;
    if encoding != "unix_bytes" {
        return Err(AxiomError::Storage(format!(
            "workspace path encoding `{encoding}` belongs to another platform"
        )));
    }
    Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
}

#[cfg(windows)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the fallback platform implementation can reject non-Unicode paths"
)]
pub(super) fn encode_path(path: &Path) -> Result<(String, Vec<u8>)> {
    use std::os::windows::ffi::OsStrExt as _;
    let bytes = path
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect();
    Ok(("windows_wtf16le".into(), bytes))
}

#[cfg(windows)]
pub(super) fn decode_path(encoding: &str, bytes: &[u8]) -> Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    if encoding != "windows_wtf16le" || !bytes.len().is_multiple_of(2) {
        return Err(AxiomError::Storage(format!(
            "invalid workspace path encoding `{encoding}`"
        )));
    }
    let wide = bytes
        .chunks_exact(2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>();
    Ok(PathBuf::from(OsString::from_wide(&wide)))
}

#[cfg(not(any(unix, windows)))]
pub(super) fn encode_path(path: &Path) -> Result<(String, Vec<u8>)> {
    let value = path.to_str().ok_or_else(|| {
        AxiomError::Storage("workspace path is not Unicode on this platform".into())
    })?;
    Ok(("utf8".into(), value.as_bytes().to_vec()))
}

#[cfg(not(any(unix, windows)))]
pub(super) fn decode_path(encoding: &str, bytes: &[u8]) -> Result<PathBuf> {
    if encoding != "utf8" {
        return Err(AxiomError::Storage(format!(
            "invalid workspace path encoding `{encoding}`"
        )));
    }
    let value = std::str::from_utf8(bytes)
        .map_err(|error| AxiomError::Storage(format!("invalid stored workspace path: {error}")))?;
    Ok(PathBuf::from(value))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "matches Result::map_err's owned error signature"
)]
pub(super) fn storage_error(error: rusqlite::Error) -> AxiomError {
    AxiomError::Storage(error.to_string())
}
