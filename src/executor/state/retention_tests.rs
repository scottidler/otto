#![cfg(test)]

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
