//! A durable turn owner backed by a process-lifetime OS lock. Lock files are
//! never removed: unlinking a live lock would allow a second inode/owner.
use super::codec::storage_error;
use crate::{AxiomError, Result, app::EventEnvelope};
use fs2::FileExt as _;
use rusqlite::{OptionalExtension as _, Transaction, params};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

pub(super) fn try_execution_lock(path: &Path, thread: &str) -> Result<Option<File>> {
    // Thread IDs are native UUIDs, never path components supplied by a client.
    let thread: crate::app::SessionId = thread
        .parse()
        .map_err(|_| AxiomError::Storage("invalid execution thread ID".into()))?;
    let mut lock_name = path.as_os_str().to_os_string();
    lock_name.push(".execution-locks");
    let directory = std::path::PathBuf::from(lock_name);
    std::fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(format!("{thread}.lock")))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(error)
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

pub(super) fn check_execution(
    transaction: &Transaction<'_>,
    envelope: &EventEnvelope,
    owner: &str,
) -> Result<Option<String>> {
    use crate::app::AppEvent;
    let thread = envelope.session_id.to_string();
    let starts = matches!(
        envelope.event,
        AppEvent::PromptAccepted { .. } | AppEvent::TurnStarted { .. }
    );
    let explicit = envelope
        .turn_id
        .as_ref()
        .or_else(|| envelope.event.turn_id())
        .map(ToString::to_string);
    if let (Some(binding), Some(event_turn)) = (&envelope.turn_id, envelope.event.turn_id())
        && binding != event_turn
    {
        return Err(conflict());
    }
    let running: Option<(String, Option<String>)> = transaction
        .query_row(
            "SELECT id, owner_id FROM turns WHERE thread_id=?1 AND status='running'",
            [&thread],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage_error)?;
    if let Some((turn, current_owner)) = &running {
        if current_owner.as_deref() != Some(owner)
            || explicit.as_ref().is_some_and(|id| id != turn)
            || matches!(envelope.event, AppEvent::PromptAccepted { .. })
        {
            return Err(conflict());
        }
    } else if explicit.is_some() && !starts {
        // Accounting can finish after turn cancellation, but must retain its
        // own exact request/turn binding and cannot mutate another execution.
        if !matches!(envelope.event, AppEvent::RequestUsageUpdated { .. }) {
            return Err(conflict());
        }
    }
    // Legacy in-process producers can omit the envelope field only while this
    // store already owns the exact turn. Never select a newest-running row.
    let resolved = explicit.or_else(|| running.map(|(turn, _)| turn));
    if let Some(turn) = &resolved
        && starts
    {
        let count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM turns WHERE thread_id=?1 AND status='running' AND id<>?2",
                params![thread, turn],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        if count != 0 {
            return Err(conflict());
        }
    }
    Ok(resolved)
}

pub(super) fn conflict() -> AxiomError {
    AxiomError::InvalidTransition(
        "conversation execution is owned by another client or turn; wait for it to finish".into(),
    )
}

pub struct ContextOperationGuard {
    store: super::SessionStore,
    thread: String,
    generation: u64,
    _lease: Option<File>,
}

impl Drop for ContextOperationGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.store.lock_state()
            && state.generation == self.generation
        {
            state.context_operations.remove(&self.thread);
        }
    }
}

impl super::SessionStore {
    /// Serialize manual compaction with inference, including while no turn row
    /// exists. The caller holds this guard through the durable context commit.
    pub fn claim_context_operation(
        &self,
        id: &crate::app::SessionId,
    ) -> Result<ContextOperationGuard> {
        let disk = self.lock()?.guard.path.is_some();
        if disk {
            self.interrupt_incomplete_turns()?;
        }
        let mut connection = self.lock()?;
        let thread = id.to_string();
        if connection.guard.context_operations.contains(&thread)
            || connection.guard.executions.contains_key(&thread)
        {
            return Err(conflict());
        }
        let lease = connection
            .guard
            .path
            .as_ref()
            .map(|path| {
                try_execution_lock(path, &thread).and_then(|lease| lease.ok_or_else(conflict))
            })
            .transpose()?;
        let running: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM turns WHERE thread_id=?1 AND status='running')",
                [&thread],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        if running {
            return Err(conflict());
        }
        let generation = connection.guard.generation;
        connection.guard.context_operations.insert(thread.clone());
        Ok(ContextOperationGuard {
            store: self.clone(),
            thread,
            generation,
            _lease: lease,
        })
    }
}
