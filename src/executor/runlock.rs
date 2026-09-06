//! The advisory lock that says a run directory belongs to a run that is still
//! going.
//!
//! `Clean` deletes run directories by path, including directories no database
//! row names, so it needs a liveness signal that does not come from the
//! database. The database cannot supply one: `record_run_start_in_db` degrades
//! silently when the store will not open, so a run can be row-less for its
//! whole duration, and `--keep-days 0` puts the cutoff at now.
//!
//! Neither can mtime. A directory's mtime moves only when its own immediate
//! entries change, and a run directory gets both of its entries (`run.yaml` and
//! `tasks/`) at init, while every later write lands in `tasks/<name>/`, two
//! levels down. Measured with nanosecond stamps: writing two levels down and
//! appending to a file in the directory both leave it unchanged. A grace period
//! keyed to it would be a floor on the run's *start* time, which is the same
//! signal retention already compares.
//!
//! So the run itself holds an exclusive advisory lock on a file inside its own
//! run directory for as long as its process lives, and cleanup skips any
//! directory whose lock is held. Two properties come from the kernel rather
//! than from otto:
//!
//! - The lock lives on the open file description and is released when the last
//!   descriptor closes, so a run killed with SIGKILL releases immediately and
//!   leaves behind no directory that can never be reclaimed.
//! - `OpenOptions` sets `FD_CLOEXEC`, so a task child does not inherit it.
//!   Liveness is the otto process, not something it spawned: an inherited lock
//!   would let a task that daemonizes a process pin its run directory forever.
//!   The one qualification: `fork` duplicates the descriptor before `exec`
//!   discards it, so a run that drops its lock while a spawn is in flight stays
//!   locked for the microseconds until that child execs. Measured. It delays a
//!   reclaim and can never lose a live run's directory, so there is nothing to
//!   build for it.

use eyre::{Context, Result};
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

/// The lock file's name inside a run directory.
pub const RUN_LOCK_NAME: &str = ".lock";

/// A run-directory lock, released when this value drops.
///
/// Whoever takes one has to keep it alive for as long as the thing it protects:
/// a run keeps it in its [`Workspace`](crate::executor::Workspace) for the life
/// of the process, and cleanup keeps it until `remove_dir_all` has returned. A
/// lock dropped after the test and before the delete protects nothing, because
/// two concurrent cleanups would then both pass the test.
#[derive(Debug)]
pub struct RunLock {
    /// `None` for a handle with nothing to release: a directory that has no
    /// lock file at all, and the in-memory filesystem used by tests.
    file: Option<File>,
}

impl RunLock {
    /// A handle that holds nothing. See the field.
    pub fn unheld() -> Self {
        Self { file: None }
    }

    /// Whether this handle holds a real kernel lock.
    pub fn is_held(&self) -> bool {
        self.file.is_some()
    }
}

/// Take the lock for a run that is starting, creating the lock file.
///
/// Called in the same step that reserves the run directory, because the
/// directory is a deletion candidate from the moment it exists.
///
/// Failing here is fatal to the run by design. Warning and proceeding would
/// leave the run's own directory eligible for deletion while it is still being
/// written to, and nothing about that would be visible until the output went
/// missing.
pub fn hold(run_dir: &Path) -> Result<RunLock> {
    let path = run_dir.join(RUN_LOCK_NAME);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("cannot open the run lock {}", path.display()))?;

    match file.try_lock() {
        Ok(()) => Ok(RunLock { file: Some(file) }),
        // Not `lock()`: this directory was created exclusively a moment ago, so
        // a holder means something is wrong rather than something is slow, and
        // blocking would hang the run's start instead of reporting it.
        Err(TryLockError::WouldBlock) => Err(eyre::eyre!(
            "another process already holds the run lock {}",
            path.display()
        )),
        Err(TryLockError::Error(e)) => Err(eyre::eyre!("cannot take the run lock {}: {e}", path.display())),
    }
}

/// Try to take the lock on an existing run directory, for a cleanup that is
/// about to delete it.
///
/// - `Ok(Some(lock))`: nothing live holds it. Hold the returned value until the
///   delete has finished.
/// - `Ok(None)`: a run is using this directory. Leave it alone.
/// - `Err`: the lock could not be tested at all. Fail closed, skip the
///   directory, and say so. `~/.otto` on NFS is the case that matters: Linux
///   emulates `flock` there through whole-file `fcntl` and an exclusive lock
///   needs write access, so the failure mode degrades to "reclaims nothing,
///   loudly" instead of "deletes a live run, silently".
///
/// **No lock file means not live, therefore deletable.** Every run directory
/// created before this existed lacks one, which is most of what the sweep is
/// for. The file is opened read-write and never created: read-only is not
/// enough for the NFS branch above, and `O_CREAT` would write into a directory
/// the caller is about to delete and would cost `--dry-run` its promise to
/// change nothing.
pub fn try_take(run_dir: &Path) -> Result<Option<RunLock>> {
    let path = run_dir.join(RUN_LOCK_NAME);
    let file = match OpenOptions::new().read(true).write(true).open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Some(RunLock::unheld())),
        Err(e) => return Err(eyre::eyre!("cannot open the run lock {}: {e}", path.display())),
    };

    match file.try_lock() {
        Ok(()) => Ok(Some(RunLock { file: Some(file) })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(eyre::eyre!("cannot test the run lock {}: {e}", path.display())),
    }
}

#[path = "runlock_tests.rs"]
mod tests;
