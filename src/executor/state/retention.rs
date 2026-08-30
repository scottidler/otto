//! Which runs a cleanup deletes.
//!
//! Pure, and deliberately shared. The SQLite store, the in-memory fake, and the
//! filesystem-scan fallback each used to carry their own copy of this logic;
//! `--keep-last` shipped inverted in the filesystem copy for exactly that
//! reason, and the fake and the real store disagreed about whether a project's
//! run count could go negative.

/// One run, reduced to the two facts retention cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunAge {
    /// Unix timestamp the run started.
    pub timestamp: u64,
    /// Whether the run ended in failure. `false` when the status is unknown,
    /// which is the filesystem-scan case.
    pub failed: bool,
}

impl From<&crate::executor::state::RunRecord> for RunAge {
    fn from(run: &crate::executor::state::RunRecord) -> Self {
        Self {
            timestamp: run.timestamp,
            failed: matches!(run.status, crate::executor::state::RunStatus::Failed),
        }
    }
}

/// The retention policy `otto Clean` applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Retention {
    /// Delete runs older than this many days.
    pub keep_days: u64,
    /// Never delete the N most recent runs, however old they are.
    pub keep_last: Option<usize>,
    /// Cutoff for failed runs, overriding `keep_days` for them.
    pub keep_failed_days: Option<u64>,
}

impl Retention {
    /// Indices into `runs` that this policy deletes, in the order they were
    /// given.
    ///
    /// `runs` may arrive in any order: the `keep_last` newest are found by
    /// timestamp, not by position, so a caller that happened to sort ascending
    /// cannot silently invert the flag.
    pub fn expired(&self, runs: &[RunAge], now: u64) -> Vec<usize> {
        let mut newest_first: Vec<usize> = (0..runs.len()).collect();
        newest_first.sort_by_key(|&i| std::cmp::Reverse(runs[i].timestamp));

        let keep_count = self.keep_last.unwrap_or(0);
        let cutoff = now.saturating_sub(self.keep_days * 24 * 60 * 60);
        let failed_cutoff = self
            .keep_failed_days
            .map(|days| now.saturating_sub(days * 24 * 60 * 60));

        let mut doomed: Vec<usize> = newest_first
            .into_iter()
            .skip(keep_count)
            .filter(|&i| {
                let run = runs[i];
                let cutoff = if run.failed { failed_cutoff.unwrap_or(cutoff) } else { cutoff };
                run.timestamp < cutoff
            })
            .collect();

        doomed.sort_unstable();
        doomed
    }
}

#[path = "retention_tests.rs"]
mod tests;
