#![cfg(test)]

use super::*;
use tempfile::TempDir;

#[test]
fn ensure_deletable_under_root_refuses_a_symlink() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path().join(".otto");
    let victim = temp_dir.path().join("victim");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&victim).unwrap();
    fs::write(victim.join("precious.txt"), "keep me").unwrap();

    let link = root.join("otto-deadbeef");
    std::os::unix::fs::symlink(&victim, &link).unwrap();

    let err = ensure_deletable_under_root(&link, &root).unwrap_err().to_string();
    assert!(err.contains("symlink"), "{err}");
    assert!(victim.join("precious.txt").exists(), "the target must be untouched");
}

#[test]
fn ensure_deletable_under_root_refuses_a_path_outside_the_root() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path().join(".otto");
    let outside = temp_dir.path().join("elsewhere");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();

    let err = ensure_deletable_under_root(&outside, &root).unwrap_err().to_string();
    assert!(err.contains("outside"), "{err}");
}

#[test]
fn ensure_deletable_under_root_refuses_the_root_itself() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path().join(".otto");
    fs::create_dir_all(&root).unwrap();

    let err = ensure_deletable_under_root(&root, &root).unwrap_err().to_string();
    assert!(err.contains("refusing to delete the otto root"), "{err}");
}

#[test]
fn ensure_deletable_under_root_accepts_a_real_run_directory() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path().join(".otto");
    let run = root.join("otto-abc123").join("1700000000");
    fs::create_dir_all(&run).unwrap();

    let canonical = ensure_deletable_under_root(&run, &root).unwrap();
    assert!(canonical.starts_with(fs::canonicalize(&root).unwrap()));
}

/// Build a project run root with a cached script and, optionally, a run
/// that symlinks to it.
fn cache_fixture(otto_home: &Path, cached: &str, referenced: bool) -> PathBuf {
    let project = otto_home.join("widget-abc12345");
    let cache = project.join(".cache");
    fs::create_dir_all(&cache).unwrap();
    let cache_entry = cache.join(cached);
    fs::write(&cache_entry, "echo hi").unwrap();

    if referenced {
        let task = project.join("1700000000").join("tasks").join("build");
        fs::create_dir_all(&task).unwrap();
        std::os::unix::fs::symlink(PathBuf::from("../../../.cache").join(cached), task.join("script.sh")).unwrap();
    }
    cache_entry
}

/// Backdate `path` past the grace period.
fn age_out(path: &Path) {
    let old = SystemTime::now() - CACHE_PRUNE_GRACE - Duration::from_secs(60);
    filetime::set_file_mtime(path, filetime::FileTime::from_system_time(old)).unwrap();
}

#[test]
fn prune_orphaned_cache_removes_an_aged_unreferenced_entry() {
    let temp_dir = TempDir::new().unwrap();
    let entry = cache_fixture(temp_dir.path(), "aaaa.sh", false);
    age_out(&entry);

    prune_orphaned_cache(temp_dir.path()).unwrap();

    assert!(!entry.exists(), "an old orphan is deleted");
}

#[test]
fn prune_orphaned_cache_spares_a_freshly_written_entry() {
    // The concurrent-run race: a run has written its cache entry but not yet
    // the symlink that references it, so it looks exactly like an orphan.
    let temp_dir = TempDir::new().unwrap();
    let entry = cache_fixture(temp_dir.path(), "bbbb.sh", false);

    prune_orphaned_cache(temp_dir.path()).unwrap();

    assert!(entry.exists(), "an entry inside the grace period must survive");
}

#[test]
fn prune_orphaned_cache_spares_a_referenced_entry() {
    let temp_dir = TempDir::new().unwrap();
    let entry = cache_fixture(temp_dir.path(), "cccc.sh", true);
    age_out(&entry);

    prune_orphaned_cache(temp_dir.path()).unwrap();

    assert!(entry.exists(), "a referenced entry is not an orphan");
}

#[test]
fn prune_orphaned_cache_ignores_directories_that_are_not_run_roots() {
    // The old `otto-` prefix test skipped every project not named "otto".
    let temp_dir = TempDir::new().unwrap();
    let stray = temp_dir.path().join("not-a-project").join(".cache");
    fs::create_dir_all(&stray).unwrap();
    let entry = stray.join("dddd.sh");
    fs::write(&entry, "echo hi").unwrap();
    age_out(&entry);

    prune_orphaned_cache(temp_dir.path()).unwrap();

    assert!(entry.exists(), "a directory that is not `<name>-<hash>` is left alone");
}

#[tokio::test]
async fn test_auto_prune_disabled() {
    let temp_dir = TempDir::new().unwrap();
    let retention = RetentionSpec {
        auto_prune: false,
        ..Default::default()
    };
    // Should return immediately without error
    auto_prune(temp_dir.path(), &retention).await;
    // .last_prune should not be created
    assert!(!temp_dir.path().join(".last_prune").exists());
}

/// The throttle's arithmetic and its comparison, at the boundary.
///
/// `cargo mutants --file src/executor/pruning.rs` left five survivors here:
/// `<` -> `==`, `<` -> `<=`, and `prune_interval_hours * 3600` -> `+` and `/`.
/// The two prune tests pinned *which* store gets pruned and nothing at all about
/// *when*, and `test_auto_prune_throttle_skip` below has no assertion in it -
/// this is the sixth test in this audit found passing for a reason unrelated to
/// what it claims.
///
/// Each row is chosen to kill a specific mutant, so the table is not padding:
///   - age 5000s under a 2h (7200s) interval must SKIP. `* -> +` makes the
///     interval 3602s and `* -> /` makes it 0s; both would prune instead.
///   - age exactly 7200s must PRUNE, because the comparison is strictly `<`.
///     `==` skips at the boundary and dies here.
/// The other two rows are the obvious directions, kept so a change that broke
/// the ordinary cases could not hide behind the boundary ones.
///
/// Verified by applying each mutant and rerunning: `==`, `+` and `/` all go red.
/// **`<` -> `<=` survives and cannot be killed from here.** The two differ only
/// when `age` equals the interval to the nanosecond, and `age` comes from
/// `modified().elapsed()`, which has always advanced a few microseconds past the
/// mtime this test set. There is no wall-clock schedule that lands on exact
/// equality. It is also a distinction without a behavioural difference: it
/// decides whether a prune happens at one exact instant. Recorded in
/// `mutants-baseline.txt` with that reason rather than left to look like an
/// oversight, and rather than contorting the test into something that pretends
/// to cover it.
///
/// The observable is the marker's mtime: `auto_prune` rewrites `.last_prune`
/// after it prunes and leaves it untouched when it skips.
#[tokio::test]
async fn auto_prune_throttle_boundary_is_exact() {
    // (label, interval_hours, marker age in seconds, must prune)
    let cases = [
        ("well inside the interval", 2u64, 100u64, false),
        ("inside, but past a mis-multiplied interval", 2, 5000, false),
        ("exactly at the interval", 2, 7200, true),
        ("past the interval", 2, 9000, true),
    ];

    for (label, hours, age_secs, must_prune) in cases {
        let temp_dir = TempDir::new().unwrap();
        let marker = temp_dir.path().join(".last_prune");
        fs::File::create(&marker).unwrap();
        let backdated = SystemTime::now() - Duration::from_secs(age_secs);
        filetime::set_file_mtime(&marker, filetime::FileTime::from_system_time(backdated)).unwrap();

        let retention = RetentionSpec {
            auto_prune: true,
            prune_interval_hours: hours,
            ..Default::default()
        };

        auto_prune(temp_dir.path(), &retention).await;

        let age_after = fs::metadata(&marker).unwrap().modified().unwrap().elapsed().unwrap();
        let pruned = age_after < Duration::from_secs(30);

        assert_eq!(
            pruned, must_prune,
            "{label}: interval {hours}h, marker aged {age_secs}s -> expected \
             prune={must_prune}, observed prune={pruned} (marker is now {age_after:?} old)"
        );
    }
}

#[tokio::test]
async fn test_auto_prune_throttle_skip() {
    let temp_dir = TempDir::new().unwrap();
    let marker = temp_dir.path().join(".last_prune");
    fs::File::create(&marker).unwrap();

    // Backdated an hour, well inside the 24h interval.
    //
    // The marker used to be created "just now" and the test then asserted
    // nothing at all - only a comment saying it "should not be re-touched
    // significantly". It could not have asserted anything: a marker written this
    // instant has the same mtime whether auto_prune skipped it or rewrote it, so
    // the skip was unobservable. Backdating makes the two outcomes distinguishable.
    let backdated = SystemTime::now() - Duration::from_secs(3600);
    let expected = filetime::FileTime::from_system_time(backdated);
    filetime::set_file_mtime(&marker, expected).unwrap();

    let retention = RetentionSpec {
        auto_prune: true,
        prune_interval_hours: 24,
        ..Default::default()
    };

    auto_prune(temp_dir.path(), &retention).await;

    assert!(marker.exists(), "the throttle must not remove the marker");
    let after = filetime::FileTime::from_last_modification_time(&fs::metadata(&marker).unwrap());
    assert_eq!(
        after.unix_seconds(),
        expected.unix_seconds(),
        "a marker inside the interval must be left exactly as it was; auto_prune rewrote it, \
         which means it pruned when it should have skipped"
    );
}

#[tokio::test]
async fn test_auto_prune_runs_when_stale() {
    let temp_dir = TempDir::new().unwrap();
    let marker = temp_dir.path().join(".last_prune");

    // Create a marker with old mtime
    fs::File::create(&marker).unwrap();
    let old_time = std::time::SystemTime::now() - Duration::from_secs(25 * 3600);
    filetime::set_file_mtime(&marker, filetime::FileTime::from_system_time(old_time)).unwrap();

    let retention = RetentionSpec {
        auto_prune: true,
        prune_interval_hours: 24,
        ..Default::default()
    };

    // Should prune and update marker
    auto_prune(temp_dir.path(), &retention).await;
    // Marker should be updated to recent time
    let meta = fs::metadata(&marker).unwrap();
    let age = meta.modified().unwrap().elapsed().unwrap();
    assert!(age < Duration::from_secs(5));
}

#[tokio::test]
async fn test_auto_prune_creates_marker_when_missing() {
    let temp_dir = TempDir::new().unwrap();
    assert!(!temp_dir.path().join(".last_prune").exists());

    let retention = RetentionSpec {
        auto_prune: true,
        prune_interval_hours: 24,
        ..Default::default()
    };

    auto_prune(temp_dir.path(), &retention).await;
    assert!(temp_dir.path().join(".last_prune").exists());
}

// =========================================================================
// Cache pruning tests
// =========================================================================

fn setup_cache_test(temp_dir: &TempDir) -> (PathBuf, PathBuf, PathBuf) {
    let project_dir = temp_dir.path().join("widget-abc12345");
    let cache_dir = project_dir.join(".cache");
    let run_dir = project_dir.join("1234567890");
    let tasks_dir = run_dir.join("tasks").join("build");

    fs::create_dir_all(&cache_dir).unwrap();
    fs::create_dir_all(&tasks_dir).unwrap();

    // Create cached scripts, aged past the grace period so they are
    // deletion candidates rather than possible in-flight writes.
    fs::write(cache_dir.join("aabb1122.sh"), "#!/bin/bash\necho hi").unwrap();
    fs::write(cache_dir.join("ccdd3344.sh"), "#!/bin/bash\necho orphan").unwrap();
    age_out(&cache_dir.join("aabb1122.sh"));
    age_out(&cache_dir.join("ccdd3344.sh"));

    // Create symlink from run to one cache entry
    #[cfg(unix)]
    std::os::unix::fs::symlink("../../../.cache/aabb1122.sh", tasks_dir.join("script.sh")).unwrap();

    (project_dir, cache_dir, run_dir)
}

#[test]
fn test_prune_orphaned_cache_removes_unreferenced() {
    let temp_dir = TempDir::new().unwrap();
    let (_project_dir, cache_dir, _run_dir) = setup_cache_test(&temp_dir);

    prune_orphaned_cache(temp_dir.path()).unwrap();

    // Referenced cache entry should remain
    assert!(cache_dir.join("aabb1122.sh").exists());
    // Orphaned cache entry should be removed
    assert!(!cache_dir.join("ccdd3344.sh").exists());
}

#[test]
fn test_prune_orphaned_cache_empty_otto_home() {
    let temp_dir = TempDir::new().unwrap();
    // Should not error on empty directory
    prune_orphaned_cache(temp_dir.path()).unwrap();
}

#[test]
fn test_prune_orphaned_cache_no_cache_dir() {
    let temp_dir = TempDir::new().unwrap();
    let project_dir = temp_dir.path().join("widget-abc12345");
    fs::create_dir_all(&project_dir).unwrap();
    // No .cache dir — should be fine
    prune_orphaned_cache(temp_dir.path()).unwrap();
}

#[test]
fn test_prune_orphaned_cache_no_runs_deletes_all() {
    let temp_dir = TempDir::new().unwrap();
    let project_dir = temp_dir.path().join("widget-abc12345");
    let cache_dir = project_dir.join(".cache");
    fs::create_dir_all(&cache_dir).unwrap();

    // Cache entries with no runs to reference them
    fs::write(cache_dir.join("aabb1122.sh"), "orphan1").unwrap();
    fs::write(cache_dir.join("ccdd3344.sh"), "orphan2").unwrap();
    age_out(&cache_dir.join("aabb1122.sh"));
    age_out(&cache_dir.join("ccdd3344.sh"));

    prune_orphaned_cache(temp_dir.path()).unwrap();

    // All cache entries should be removed
    assert!(!cache_dir.join("aabb1122.sh").exists());
    assert!(!cache_dir.join("ccdd3344.sh").exists());
}
