//! What a failing auto-prune tells the user.
//!
//! `auto_prune` runs on the way out of every `otto <task>` and reports its own
//! failures with `warn!`, which `setup_logging` sends to `otto.log`. The only
//! thing that reached the terminal was one line per refused run, printed by
//! `Clean` itself and naming neither auto-prune nor what the failure costs, so
//! run directories accumulating under `$OTTO_HOME` had no visible cause.

mod common;

use common::otto_cmd;
use predicates::prelude::*;
use serial_test::serial;
use std::fs;
use tempfile::TempDir;

/// `sleep 1` so the seeded run is a second old by the time the second run's
/// prune asks: `keep_days: 0` selects runs strictly older than now, and two
/// runs in the same second are not.
const OTTOFILE: &str = r#"
otto:
  api: 1
  retention:
    keep_days: 0
    keep_last: 0
    keep_failed: 0
    auto_prune: false
    prune_interval_hours: 0
tasks:
  noop:
    bash: sleep 1
"#;

#[test]
#[serial]
fn a_failing_auto_prune_says_so_on_stderr_and_says_what_it_costs() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join(".otto");
    let ottofile = temp.path().join("otto.yml");
    fs::write(&ottofile, OTTOFILE).expect("write ottofile");

    // Seed one run with auto-prune off, so the row survives its own run.
    otto_cmd(&home).current_dir(temp.path()).arg("noop").assert().success();

    // Replace that run's directory with a symlink. `delete_run` refuses to
    // delete through one (`ensure_deletable_under_root`), which is a real
    // refusal rather than a simulated one.
    let victim = temp.path().join("victim");
    fs::create_dir_all(&victim).expect("victim");
    let run_dir = fs::read_dir(&home)
        .expect("read home")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .flat_map(|project| fs::read_dir(project).expect("read project").flatten())
        .map(|entry| entry.path())
        .find(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('1'))
        })
        .expect("the seeded run wrote a run directory");
    fs::remove_dir_all(&run_dir).expect("remove run dir");
    std::os::unix::fs::symlink(&victim, &run_dir).expect("symlink");

    // Now turn auto-prune on and run again: its prune selects the sabotaged
    // run and cannot delete it.
    fs::write(&ottofile, OTTOFILE.replace("auto_prune: false", "auto_prune: true")).expect("rewrite ottofile");

    otto_cmd(&home)
        .current_dir(temp.path())
        .arg("noop")
        .assert()
        .success()
        .stderr(predicate::str::contains("auto-prune failed"))
        .stderr(predicate::str::contains("are not being cleaned up"));

    assert!(victim.exists(), "the symlink target must survive the refused delete");
}
