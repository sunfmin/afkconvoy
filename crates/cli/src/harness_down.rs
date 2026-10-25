//! Structured send evidence is routed to scoped worker health diagnosis.
use crate::orca::Worktree;
use anyhow::Result;

pub(crate) const PROBE_SECONDS: u64 = crate::health::CHECK_SECONDS;
pub fn tell(
    worktree: &Worktree,
    _terminal: &str,
    _now: u64,
    send: impl FnOnce() -> Result<()>,
) -> Result<()> {
    match send() {
        Ok(()) => {
            let file = worktree.note("convoy-worker-error.json")?;
            if file.exists() {
                std::fs::remove_file(file)?;
            }
            Ok(())
        }
        Err(error) => {
            if let Some(blocked) = error.downcast_ref::<crate::orca::PromptBlocked>() {
                crate::health::signal(worktree, &blocked.0)?;
            }
            Err(error)
        }
    }
}
