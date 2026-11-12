//! Local migration observations contain phase/counts only, never chat data.
pub(crate) use axiom_acp_extension::{
    StorageMigrationPhase as Phase, StorageMigrationProgress as Progress,
};

pub(crate) type Observer<'a> = dyn Fn(Progress) + 'a;

pub(crate) fn phase(observer: &Observer<'_>, phase: Phase) {
    observer(Progress {
        phase,
        completed: None,
        total: None,
    });
}

pub(crate) fn count(observer: &Observer<'_>, phase: Phase, completed: u64, total: u64) {
    // Bound notification volume even for very large histories, retaining exact
    // phase boundaries and the final count. ACP coalesces slow consumers too.
    if completed == 0 || completed == total || completed.is_multiple_of(total.div_ceil(100).max(1))
    {
        observer(Progress {
            phase,
            completed: Some(completed),
            total: Some(total),
        });
    }
}
