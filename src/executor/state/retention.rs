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

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 24 * 60 * 60;

    fn aged(days_ago: u64, now: u64) -> RunAge {
        RunAge {
            timestamp: now - days_ago * DAY,
            failed: false,
        }
    }

    #[test]
    fn test_keep_days_deletes_only_the_old() {
        let now = 100 * DAY;
        let runs = [aged(40, now), aged(10, now)];
        let policy = Retention {
            keep_days: 30,
            ..Default::default()
        };
        assert_eq!(policy.expired(&runs, now), vec![0]);
    }

    #[test]
    fn test_keep_last_keeps_the_newest_not_the_oldest() {
        // The inversion that shipped: an ascending list plus `split_off` kept
        // the oldest two and deleted everything newer.
        let now = 100 * DAY;
        let runs = [
            aged(44, now),
            aged(43, now),
            aged(42, now),
            aged(41, now),
            aged(40, now),
        ];
        let policy = Retention {
            keep_days: 30,
            keep_last: Some(2),
            ..Default::default()
        };
        // Indices 3 and 4 are the two newest, so 0..=2 go.
        assert_eq!(policy.expired(&runs, now), vec![0, 1, 2]);
    }

    #[test]
    fn test_keep_last_survives_an_ascending_caller() {
        let now = 100 * DAY;
        let mut runs = [aged(40, now), aged(44, now), aged(42, now)];
        runs.sort_by_key(|r| r.timestamp); // oldest first
        let policy = Retention {
            keep_days: 30,
            keep_last: Some(1),
            ..Default::default()
        };
        let doomed = policy.expired(&runs, now);
        let survivor: Vec<u64> = (0..runs.len())
            .filter(|i| !doomed.contains(i))
            .map(|i| runs[i].timestamp)
            .collect();
        assert_eq!(survivor, vec![now - 40 * DAY], "the newest run must survive");
    }

    #[test]
    fn test_keep_last_larger_than_the_list_deletes_nothing() {
        let now = 100 * DAY;
        let runs = [aged(40, now), aged(41, now)];
        let policy = Retention {
            keep_days: 0,
            keep_last: Some(5),
            ..Default::default()
        };
        assert!(policy.expired(&runs, now).is_empty());
    }

    #[test]
    fn test_keep_failed_days_extends_retention_for_failures() {
        let now = 100 * DAY;
        let runs = [
            RunAge {
                timestamp: now - 40 * DAY,
                failed: true,
            },
            aged(40, now),
        ];
        let policy = Retention {
            keep_days: 30,
            keep_last: None,
            keep_failed_days: Some(45),
        };
        assert_eq!(policy.expired(&runs, now), vec![1], "only the successful run expires");
    }

    #[test]
    fn test_empty_input_is_empty_output() {
        assert!(Retention::default().expired(&[], 0).is_empty());
    }
}
