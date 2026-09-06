//! Concurrent processes racing a cold database must all persist their runs.
//!
//! Found by the batched audit, batch 5 of 14. Two separate races sat on the
//! first-open path, both of which lost runs silently at exit 0 with nothing on
//! stderr - the failure only ever reached otto's log file. See
//! `docs/design/2026-06-10-code-review-remediation.md` Phase 4.
//!
//! The *upgrade* path is the same race one version later: an existing database
//! behind the current schema, opened by several processes at once. See
//! `docs/design/2026-09-02-second-code-review-remediation.md` Phase 9.

mod common;

use common::otto_std_cmd;
use std::fs;
use std::path::Path;

use tempfile::TempDir;

const OTTOFILE: &str = "otto:\n  api: 1\n  tasks: [t]\ntasks:\n  t:\n    action: echo ran\n";

/// Count the `runs` rows a cold-start burst persisted, by asking otto itself:
/// `History` reads the same rows, through the same code a user would.
fn persisted_runs(project: &Path, home: &Path) -> usize {
    let output = otto_std_cmd(home)
        .current_dir(project)
        .env_remove("OTTOFILE")
        .args(["History"])
        .output()
        .expect("failed to run otto History");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("Total runs:"))
        .unwrap_or_else(|| panic!("otto History printed no total; stdout was:\n{stdout}"))
        .trim()
        .parse()
        .expect("the total must be a number")
}

/// N processes starting against a database that does not exist yet must produce
/// N run records.
///
/// Two defects lived here. `migrations.rs`'s `current_version == 0` branch had no
/// transaction while all four upgrade branches had one, so every racer ran
/// `init_schema` and then `INSERT`ed the same schema version, and the losers died
/// on `UNIQUE constraint failed: schema_version.version`. Underneath that,
/// `db.rs` set `busy_timeout` *after* the `journal_mode=WAL` pragma, so the one
/// statement needing a brief exclusive lock ran with no timeout at all and the
/// losers died on `Failed to enable WAL mode: database is locked`.
///
/// Measured before the fix: five concurrent cold starts persisted as few as 1 of
/// 5, and every process still exited 0 with empty stderr.
#[test]
fn concurrent_cold_starts_all_persist_their_runs() {
    const RACERS: usize = 8;

    // Repeated, because both defects were races: a single pass passed by luck
    // often enough to look green.
    for trial in 1..=5 {
        let dir = TempDir::new().expect("tempdir");
        let project = dir.path().join("project");
        let home = dir.path().join("otto-home");
        fs::create_dir_all(&project).expect("create project");
        fs::write(project.join("otto.yml"), OTTOFILE).expect("write ottofile");
        // Deliberately NOT creating `home`: the database file must not exist, which
        // is the whole point of a cold start.

        let mut children = Vec::new();
        for _ in 0..RACERS {
            let child = otto_std_cmd(&home)
                .current_dir(&project)
                .env_remove("OTTOFILE")
                .args(["t"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("failed to spawn otto");
            children.push(child);
        }

        for (i, child) in children.into_iter().enumerate() {
            let out = child.wait_with_output().expect("failed to wait for otto");
            assert!(
                out.status.success(),
                "trial {trial} racer {i} exited {}\nstdout:\n{}\nstderr:\n{}",
                out.status,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }

        let persisted = persisted_runs(&project, &home);
        assert_eq!(
            persisted, RACERS,
            "trial {trial}: {persisted} of {RACERS} concurrent cold-start runs persisted; \
             the rest were lost silently at exit 0"
        );
    }
}

/// Runs that start in the same second must not share a run directory.
///
/// `layout.rs` named the run directory after its start timestamp alone. Once
/// Phase 4 dropped `UNIQUE(runs.timestamp)` so the rows stopped colliding, N
/// same-second runs became N rows pointing at ONE directory: they raced creating
/// it, overwrote each other's task output, and cleaning any one of them deleted
/// the directory the rest still referenced. The criterion "two runs started in
/// the same second both persist" was true of the rows and false of their
/// artifacts.
///
/// Found by the batched audit, batch 5 of 14.
#[test]
fn same_second_runs_get_their_own_directories() {
    const RACERS: usize = 6;

    let dir = TempDir::new().expect("tempdir");
    let project = dir.path().join("project");
    let home = dir.path().join("otto-home");
    fs::create_dir_all(&project).expect("create project");
    fs::create_dir_all(&home).expect("create home");
    fs::write(project.join("otto.yml"), OTTOFILE).expect("write ottofile");

    // Warm the database first, so this test fails only on the directory
    // collision and not on the cold-start race the test above owns.
    otto_std_cmd(&home)
        .current_dir(&project)
        .env_remove("OTTOFILE")
        .args(["t"])
        .output()
        .expect("warm-up run");

    let mut children = Vec::new();
    for _ in 0..RACERS {
        children.push(
            otto_std_cmd(&home)
                .current_dir(&project)
                .env_remove("OTTOFILE")
                .args(["t"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("failed to spawn otto"),
        );
    }
    for (i, child) in children.into_iter().enumerate() {
        let out = child.wait_with_output().expect("failed to wait for otto");
        assert!(
            out.status.success(),
            "racer {i} exited {}\nstderr:\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // One project root, and every run underneath it holds its own directory.
    let project_root = fs::read_dir(&home)
        .expect("read otto home")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().contains('-')))
        .expect("a project run root must exist");

    let run_dirs: Vec<String> = fs::read_dir(&project_root)
        .expect("read project root")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n != ".cache")
        .collect();

    let mut unique = run_dirs.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        run_dirs.len(),
        "run directory names must be unique, got: {run_dirs:?}"
    );
    assert_eq!(
        run_dirs.len(),
        RACERS + 1,
        "each of the {RACERS} concurrent runs plus the warm-up needs its own directory, got: {run_dirs:?}"
    );
}

/// A schema-v4 database, the shape otto 2.2 upgrades from: both skip columns
/// present on `tasks`, `runs` still carrying `UNIQUE(timestamp)` and no
/// `run_dir`. Written with rusqlite rather than through otto, because otto can
/// only ever produce the current version.
fn write_v4_fixture(db_path: &Path) {
    let conn = rusqlite::Connection::open(db_path).expect("open the fixture database");
    conn.execute_batch(
        "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);
         CREATE TABLE projects (
            id INTEGER PRIMARY KEY,
            hash TEXT NOT NULL UNIQUE,
            name TEXT,
            ottofile_path TEXT,
            first_seen INTEGER NOT NULL,
            last_seen INTEGER NOT NULL,
            run_count INTEGER DEFAULT 0
         );
         CREATE TABLE runs (
            id INTEGER PRIMARY KEY,
            project_id INTEGER NOT NULL,
            timestamp INTEGER NOT NULL UNIQUE,
            status TEXT NOT NULL,
            duration_seconds REAL,
            size_bytes INTEGER,
            ottofile_path TEXT,
            cwd TEXT,
            user TEXT,
            hostname TEXT,
            args TEXT,
            ended_at INTEGER,
            FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
         );
         CREATE TABLE tasks (
            id INTEGER PRIMARY KEY,
            run_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            status TEXT NOT NULL,
            script_hash TEXT,
            exit_code INTEGER,
            started_at INTEGER,
            ended_at INTEGER,
            duration_seconds REAL,
            stdout_path TEXT,
            stderr_path TEXT,
            script_path TEXT,
            skip_reason TEXT,
            skip_kind TEXT,
            FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE CASCADE
         );
         INSERT INTO schema_version (version, applied_at) VALUES (4, 1700000000);
         INSERT INTO projects (hash, name, first_seen, last_seen, run_count)
              VALUES ('feedface', 'legacy', 1700000000, 1700000000, 1);
         INSERT INTO runs (id, project_id, timestamp, status)
              VALUES (1, 1, 1700000000, 'success');
         INSERT INTO tasks (run_id, name, status) VALUES (1, 'legacy-task', 'completed');",
    )
    .expect("write the v4 fixture");
}

/// N processes opening a database that is behind the current schema must all
/// succeed, and the pre-existing rows must survive the upgrade.
///
/// The cold-start branch took `BEGIN IMMEDIATE` and documented why; all four
/// upgrade branches used `conn.unchecked_transaction()`, which is deferred. A
/// deferred transaction starts read-only, and every upgrade step reads before it
/// writes (`column_exists`, then `ALTER TABLE`), so the losers of the race asked
/// SQLite to upgrade a read lock to a write lock - which SQLite refuses to make
/// wait, returning SQLITE_BUSY at once no matter what `busy_timeout` says.
///
/// The v4-to-v5 step is the sharp one: it drops and rebuilds `runs`, so a loser
/// that got partway through would take the run history with it.
#[test]
fn concurrent_upgrades_from_an_older_schema_all_succeed() {
    const RACERS: usize = 4;

    // Repeated, because this is a race: one pass proves very little.
    for trial in 1..=5 {
        let dir = TempDir::new().expect("tempdir");
        let project = dir.path().join("project");
        let home = dir.path().join("otto-home");
        fs::create_dir_all(&project).expect("create project");
        fs::create_dir_all(&home).expect("create home");
        fs::write(project.join("otto.yml"), OTTOFILE).expect("write ottofile");
        write_v4_fixture(&home.join("otto.db"));

        let mut children = Vec::new();
        for _ in 0..RACERS {
            children.push(
                otto_std_cmd(&home)
                    .current_dir(&project)
                    .env_remove("OTTOFILE")
                    .args(["t"])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .expect("failed to spawn otto"),
            );
        }

        for (i, child) in children.into_iter().enumerate() {
            let out = child.wait_with_output().expect("failed to wait for otto");
            assert!(
                out.status.success(),
                "trial {trial} racer {i} exited {}\nstdout:\n{}\nstderr:\n{}",
                out.status,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }

        // The fixture's one legacy run plus one per racer: the upgrade neither
        // lost the old rows nor lost a concurrent run to SQLITE_BUSY.
        let persisted = persisted_runs(&project, &home);
        assert_eq!(
            persisted,
            RACERS + 1,
            "trial {trial}: {persisted} runs persisted across a concurrent upgrade, expected {}",
            RACERS + 1
        );

        let conn = rusqlite::Connection::open(home.join("otto.db")).expect("reopen the upgraded database");
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| row.get(0))
            .expect("read the schema version");
        assert_eq!(
            version, 5,
            "trial {trial}: the database must land on the current schema"
        );
        let legacy_tasks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tasks t JOIN runs r ON t.run_id = r.id WHERE t.name = 'legacy-task'",
                [],
                |row| row.get(0),
            )
            .expect("count the legacy task");
        assert_eq!(
            legacy_tasks, 1,
            "trial {trial}: the v4-to-v5 rebuild of `runs` must not orphan the tasks that reference it"
        );
    }
}
