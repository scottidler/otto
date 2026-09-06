use eyre::{Context, Result};
use rusqlite::{OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::db::{DatabaseManager, TransactionGuard};
use super::metadata::RunMetadata;
use super::retention::{Retention, RunAge};
use super::schema::{RunStatus, SkipKind, TaskStatus};
use crate::ports::StateStore;

/// Every column of `runs`, in the order [`StateManager::row_to_run_record`]
/// reads them. One list, because a query that forgot a column used to hand the
/// mapper the wrong field.
const RUN_COLUMNS: &str = "r.id, r.project_id, r.timestamp, r.status, r.duration_seconds,
     r.size_bytes, r.ottofile_path, r.cwd, r.user, r.hostname, r.args, r.ended_at, r.run_dir";

/// Report a column whose stored value does not fit the type the code expects,
/// instead of quietly substituting a default.
fn bad_column(index: usize, detail: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, detail)),
    )
}

/// State manager for recording and querying run/task state
pub struct StateManager {
    db: DatabaseManager,
}

/// A run record from the database
#[derive(Debug, Clone, serde::Serialize)]
pub struct RunRecord {
    pub id: i64,
    pub project_id: i64,
    pub timestamp: u64,
    pub status: RunStatus,
    pub duration_seconds: Option<f64>,
    pub size_bytes: Option<u64>,
    pub ottofile_path: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub user: Option<String>,
    pub hostname: Option<String>,
    pub args: Option<Vec<String>>,
    pub ended_at: Option<u64>,
    /// The directory this run wrote into, as recorded at run start. `None` for
    /// rows written before schema v5, and for runs whose start was recorded
    /// without one; no path is derived for those, and `Clean` reclaims the
    /// directory by path instead.
    pub run_dir: Option<PathBuf>,
}

/// A task record from the database
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskRecord {
    pub id: i64,
    pub run_id: i64,
    pub name: String,
    pub status: TaskStatus,
    pub script_hash: Option<String>,
    pub exit_code: Option<i32>,
    pub started_at: Option<u64>,
    pub ended_at: Option<u64>,
    pub duration_seconds: Option<f64>,
    pub stdout_path: Option<PathBuf>,
    pub stderr_path: Option<PathBuf>,
    pub script_path: Option<PathBuf>,
    /// Human-readable detail naming the edge or ordering gate that skipped this
    /// task, for skips caused by an unreachable dependency edge or a
    /// serial-group cascade. `None` for every other status and for an
    /// up-to-date skip, which is a success, not a gated-out task.
    pub skip_reason: Option<String>,
    /// The typed provenance behind `skip_reason`. Same population rule; this is
    /// the machine-readable half, so history can be filtered by reason class
    /// instead of by substring.
    pub skip_kind: Option<SkipKind>,
}

/// Overall system statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct OverallStats {
    pub total_runs: u64,
    pub successful_runs: u64,
    pub failed_runs: u64,
    pub running_runs: u64,
    pub total_tasks: u64,
    pub total_disk_usage: u64,
    pub total_duration_seconds: f64,
}

/// Task-specific statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskStats {
    pub project_id: i64,
    pub project_hash: String,
    pub project_name: String,
    pub task_name: String,
    pub total_executions: u64,
    pub successful_executions: u64,
    pub failed_executions: u64,
    pub skipped_executions: u64,
    pub avg_duration_seconds: Option<f64>,
    pub min_duration_seconds: Option<f64>,
    pub max_duration_seconds: Option<f64>,
    pub last_executed: Option<u64>,
    pub last_status: Option<TaskStatus>,
}

/// Project summary information
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectSummary {
    pub id: i64,
    pub hash: String,
    pub name: String,
    pub ottofile_path: Option<PathBuf>,
    pub run_count: u64,
    pub last_seen: u64,
}

impl StateManager {
    pub fn new() -> Result<Self> {
        let db = DatabaseManager::open_default()?;
        Ok(Self { db })
    }

    pub fn with_db_path(db_path: PathBuf) -> Result<Self> {
        let db = DatabaseManager::new(db_path)?;
        Ok(Self { db })
    }

    /// Try to create a StateManager, returning None if the database is
    /// unavailable, so a run still executes without its history.
    ///
    /// Degrading is fine; degrading silently is not. This used to be
    /// `Self::new().ok()`, which threw away the only explanation of why history,
    /// stats, and DB-driven cleanup had all stopped working.
    pub fn try_new() -> Option<Self> {
        match Self::new() {
            Ok(manager) => Some(manager),
            Err(e) => {
                log::warn!("State database unavailable, continuing without run history: {e:#}");
                None
            }
        }
    }

    /// Record the start of a run: the project upsert, the run row, and the
    /// project's run counter.
    ///
    /// All three in one immediate transaction. They used to be four autocommit
    /// statements, so a crash - or a failure - between the run INSERT and the
    /// counter UPDATE drifted `projects.run_count` permanently against the rows
    /// in `runs`, in either direction. The `MAX(run_count - 1, 0)` guard in
    /// [`Self::delete_run`] exists because of exactly that drift.
    pub fn record_run_start(&self, metadata: &RunMetadata) -> Result<i64> {
        log::debug!(
            "record_run_start: hash={} timestamp={}",
            metadata.hash,
            metadata.timestamp
        );
        self.db.with_connection(|conn| {
            // Read-then-write (the upsert reads back the project id, the counter
            // reads the current count), so the write lock is taken up front.
            let tx = TransactionGuard::immediate(conn)?;

            // First, ensure project exists
            let project_id = self.ensure_project(conn, &metadata.hash, metadata.ottofile.as_ref())?;

            // Serialize args if present
            let args_json = metadata
                .args
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .context("Failed to serialize args")?;

            // Insert run record
            conn.execute(
                "INSERT INTO runs (
                    project_id, timestamp, status, ottofile_path, cwd, user, hostname, args, run_dir
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    project_id,
                    metadata.timestamp as i64,
                    RunStatus::Running.as_str(),
                    metadata.ottofile.as_ref().map(|p| p.to_string_lossy().to_string()),
                    metadata.cwd.as_ref().map(|p| p.to_string_lossy().to_string()),
                    metadata.user,
                    metadata.hostname,
                    args_json,
                    metadata.run_dir.as_ref().map(|p| p.to_string_lossy().to_string()),
                ],
            )?;

            let run_id = conn.last_insert_rowid();

            conn.execute(
                "UPDATE projects
                 SET last_seen = ?1, run_count = run_count + 1
                 WHERE id = ?2",
                params![metadata.timestamp as i64, project_id],
            )?;

            tx.commit()?;
            Ok(run_id)
        })
    }

    /// Record the completion of a run.
    ///
    /// Keyed on `run_id`, not on the timestamp: timestamps have one-second
    /// resolution, so two runs started in the same second used to complete each
    /// other. The duration is computed in SQL from the row's own start time.
    pub fn record_run_complete(&self, run_id: i64, status: RunStatus, size_bytes: Option<u64>) -> Result<()> {
        log::debug!("record_run_complete: run_id={run_id} status={status:?} size_bytes={size_bytes:?}");
        let ended_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("Failed to get current time")?
            .as_secs();

        self.db.with_connection(|conn| {
            let updated = conn.execute(
                "UPDATE runs
                 SET status = ?1, size_bytes = ?2, ended_at = ?3,
                     duration_seconds = MAX(?3 - timestamp, 0)
                 WHERE id = ?4",
                params![status.as_str(), size_bytes.map(|s| s as i64), ended_at as i64, run_id],
            )?;

            // A completion for a run that is not there is a lost run, not a
            // no-op: the row stays `running` forever and skews every stat.
            if updated == 0 {
                return Err(eyre::eyre!("No run with id {run_id} to mark complete"));
            }
            Ok(())
        })
    }

    pub fn record_task_start(
        &self,
        run_id: i64,
        task_name: &str,
        script_hash: Option<&str>,
        stdout_path: Option<&PathBuf>,
        stderr_path: Option<&PathBuf>,
        script_path: Option<&PathBuf>,
    ) -> Result<i64> {
        log::debug!("record_task_start: run_id={run_id} task_name={task_name} script_hash={script_hash:?}");
        let started_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("Failed to get current time")?
            .as_secs();

        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO tasks (
                    run_id, name, status, script_hash, started_at,
                    stdout_path, stderr_path, script_path
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    run_id,
                    task_name,
                    TaskStatus::Running.as_str(),
                    script_hash,
                    started_at as i64,
                    stdout_path.map(|p| p.to_string_lossy().to_string()),
                    stderr_path.map(|p| p.to_string_lossy().to_string()),
                    script_path.map(|p| p.to_string_lossy().to_string()),
                ],
            )?;

            Ok(conn.last_insert_rowid())
        })
    }

    /// Record the completion of a task.
    ///
    /// One statement, the same shape as [`Self::record_run_complete`]: the
    /// duration is computed in SQL from the row's own start time and clamped at
    /// zero. It used to be a `SELECT started_at` followed by an `UPDATE`, with
    /// the subtraction done in Rust and unclamped - so a clock that stepped
    /// backwards between start and end wrote a negative duration into the stats,
    /// and a concurrent delete between the two statements turned the completion
    /// into a silent no-op. `started_at` is nullable, hence the `CASE`: a task
    /// with no recorded start has no duration, not a duration measured from the
    /// epoch.
    pub fn record_task_complete(&self, task_id: i64, exit_code: i32, status: TaskStatus) -> Result<()> {
        log::debug!("record_task_complete: task_id={task_id} exit_code={exit_code} status={status:?}");
        let ended_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("Failed to get current time")?
            .as_secs();

        self.db.with_connection(|conn| {
            let updated = conn.execute(
                "UPDATE tasks
                 SET status = ?1, exit_code = ?2, ended_at = ?3,
                     duration_seconds = CASE
                         WHEN started_at IS NULL THEN NULL
                         ELSE MAX(?3 - started_at, 0)
                     END
                 WHERE id = ?4",
                params![status.as_str(), exit_code, ended_at as i64, task_id],
            )?;

            if updated == 0 {
                return Err(eyre::eyre!("No task with id {task_id} to mark complete"));
            }
            Ok(())
        })
    }

    /// Record that a task was skipped, and why.
    ///
    /// `skip_kind` is the typed provenance the scheduler branched on and
    /// `skip_reason` is the sentence it rendered for the operator; both come
    /// from one `SkipRecord`, so they cannot disagree.
    pub fn record_task_skipped(
        &self,
        run_id: i64,
        task_name: &str,
        script_hash: Option<&str>,
        skip_reason: Option<&str>,
        skip_kind: Option<SkipKind>,
    ) -> Result<i64> {
        log::debug!("record_task_skipped: run_id={run_id} task_name={task_name} skip_kind={skip_kind:?}");
        // A skip happens at a moment, and callers that order a run's tasks by
        // `started_at` need one: without it, every skipped task sorted ahead
        // of the run it belongs to.
        let started_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("Failed to get current time")?
            .as_secs();

        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO tasks (run_id, name, status, script_hash, started_at, skip_reason, skip_kind)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    run_id,
                    task_name,
                    TaskStatus::Skipped.as_str(),
                    script_hash,
                    started_at as i64,
                    skip_reason,
                    skip_kind.map(|k| k.as_str())
                ],
            )?;

            Ok(conn.last_insert_rowid())
        })
    }

    pub fn get_task_history(&self, task_name: &str, limit: usize) -> Result<Vec<TaskRecord>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, run_id, name, status, script_hash, exit_code,
                        started_at, ended_at, duration_seconds,
                        stdout_path, stderr_path, script_path, skip_reason, skip_kind
                 FROM tasks
                 WHERE name = ?1
                 ORDER BY started_at DESC
                 LIMIT ?2",
            )?;

            let rows = stmt.query_map(params![task_name, limit as i64], Self::row_to_task_record)?;

            rows.collect::<Result<Vec<_>, _>>()
                .context("Failed to fetch task history")
        })
    }

    /// Helper to convert a database row to a RunRecord
    ///
    /// Unreadable stored values are reported, not smoothed over: corrupt args
    /// used to become `None` and an unknown status used to become `Failed`, so a
    /// damaged row read back as a plausible, wrong one.
    fn row_to_run_record(row: &rusqlite::Row) -> rusqlite::Result<RunRecord> {
        let args_json: Option<String> = row.get(10)?;
        let args = args_json
            .map(|json| {
                serde_json::from_str::<Vec<String>>(&json)
                    .map_err(|e| bad_column(10, format!("run args are not a JSON string list: {e}")))
            })
            .transpose()?;

        let status_str: String = row.get(3)?;
        let status =
            RunStatus::parse(&status_str).ok_or_else(|| bad_column(3, format!("unknown run status {status_str:?}")))?;

        Ok(RunRecord {
            id: row.get(0)?,
            project_id: row.get(1)?,
            timestamp: row.get::<_, i64>(2)? as u64,
            status,
            duration_seconds: row.get(4)?,
            size_bytes: row.get::<_, Option<i64>>(5)?.map(|s| s as u64),
            ottofile_path: row.get::<_, Option<String>>(6)?.map(PathBuf::from),
            cwd: row.get::<_, Option<String>>(7)?.map(PathBuf::from),
            user: row.get(8)?,
            hostname: row.get(9)?,
            args,
            ended_at: row.get::<_, Option<i64>>(11)?.map(|t| t as u64),
            run_dir: row.get::<_, Option<String>>(12)?.map(PathBuf::from),
        })
    }

    /// Helper to convert a database row to a TaskRecord
    ///
    /// Same rule as [`Self::row_to_run_record`] for every stored enum: an
    /// unreadable value is reported, not smoothed over. `skip_kind` used to go
    /// through `and_then(SkipKind::parse)`, so an unrecognised kind read back as
    /// "this task was skipped for no recorded reason" while the status column
    /// next to it rejected the same corruption.
    fn row_to_task_record(row: &rusqlite::Row) -> rusqlite::Result<TaskRecord> {
        let status_str: String = row.get(3)?;
        let status = TaskStatus::parse(&status_str)
            .ok_or_else(|| bad_column(3, format!("unknown task status {status_str:?}")))?;

        let skip_kind = match row.get::<_, Option<String>>(13)? {
            Some(kind_str) => Some(
                SkipKind::parse(&kind_str).ok_or_else(|| bad_column(13, format!("unknown skip kind {kind_str:?}")))?,
            ),
            None => None,
        };

        Ok(TaskRecord {
            id: row.get(0)?,
            run_id: row.get(1)?,
            name: row.get(2)?,
            status,
            script_hash: row.get(4)?,
            exit_code: row.get(5)?,
            started_at: row.get::<_, Option<i64>>(6)?.map(|t| t as u64),
            ended_at: row.get::<_, Option<i64>>(7)?.map(|t| t as u64),
            duration_seconds: row.get(8)?,
            stdout_path: row.get::<_, Option<String>>(9)?.map(PathBuf::from),
            stderr_path: row.get::<_, Option<String>>(10)?.map(PathBuf::from),
            script_path: row.get::<_, Option<String>>(11)?.map(PathBuf::from),
            skip_reason: row.get(12)?,
            skip_kind,
        })
    }

    /// Get overall system statistics
    pub fn get_overall_stats(&self) -> Result<OverallStats> {
        self.db.with_connection(|conn| {
            let total_runs: u64 = conn.query_row("SELECT COUNT(*) FROM runs", [], |row| row.get::<_, i64>(0))? as u64;

            let successful_runs: u64 =
                conn.query_row("SELECT COUNT(*) FROM runs WHERE status = 'success'", [], |row| {
                    row.get::<_, i64>(0)
                })? as u64;

            let failed_runs: u64 = conn.query_row("SELECT COUNT(*) FROM runs WHERE status = 'failed'", [], |row| {
                row.get::<_, i64>(0)
            })? as u64;

            let running_runs: u64 = conn.query_row("SELECT COUNT(*) FROM runs WHERE status = 'running'", [], |row| {
                row.get::<_, i64>(0)
            })? as u64;

            let total_tasks: u64 = conn.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get::<_, i64>(0))? as u64;

            // These used to be `unwrap_or(0)` / `unwrap_or(0.0)`: a broken stats
            // query reported a healthy zero instead of an error, so "no disk
            // used" and "cannot read the database" looked identical.
            let total_disk_usage: i64 =
                conn.query_row("SELECT COALESCE(SUM(size_bytes), 0) FROM runs", [], |row| row.get(0))?;

            let total_duration_seconds: f64 =
                conn.query_row("SELECT COALESCE(SUM(duration_seconds), 0) FROM runs", [], |row| {
                    row.get(0)
                })?;

            Ok(OverallStats {
                total_runs,
                successful_runs,
                failed_runs,
                running_runs,
                total_tasks,
                total_disk_usage: total_disk_usage as u64,
                total_duration_seconds,
            })
        })
    }

    /// Get all projects with summary information
    pub fn get_all_projects(&self) -> Result<Vec<ProjectSummary>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, hash, name, ottofile_path, run_count, last_seen
                 FROM projects
                 ORDER BY last_seen DESC",
            )?;

            let projects = stmt
                .query_map([], |row| {
                    // `name` is nullable until the v1-to-v2 backfill has run, so
                    // it falls back to the hash. The fallback used to re-read the
                    // hash column and `unwrap()` it inside the mapper.
                    let hash: String = row.get(1)?;
                    let name: Option<String> = row.get(2)?;
                    Ok(ProjectSummary {
                        id: row.get(0)?,
                        name: name.unwrap_or_else(|| hash.clone()),
                        hash,
                        ottofile_path: row.get::<_, Option<String>>(3)?.map(PathBuf::from),
                        run_count: row.get::<_, i64>(4)? as u64,
                        last_seen: row.get::<_, i64>(5)? as u64,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(projects)
        })
    }

    /// Surfaced, not nulled: an unrecognised status is a named error, not "this
    /// task has no last status". Same rule as `row_to_task_record`.
    fn parse_last_status(status: Option<String>) -> rusqlite::Result<Option<TaskStatus>> {
        status
            .map(|s| {
                TaskStatus::parse(&s)
                    .ok_or_else(|| bad_column(0, format!("unknown task status {s:?} in the task stats query")))
            })
            .transpose()
    }

    /// Get statistics for a specific task across all projects
    ///
    /// One `GROUP BY` query per task, not eight queries per project: the
    /// per-project loop used to re-scan `tasks JOIN runs` once for the count,
    /// once per status, and three times for durations. `last_status` is the
    /// one field a plain aggregate cannot give the correlated row for, since
    /// the query already has two other `MIN`/`MAX` aggregates in play; it is
    /// the only correlated subquery left.
    pub fn get_task_stats(&self, task_name: &str) -> Result<Vec<TaskStats>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT p.id, p.hash, p.name,
                        COUNT(*),
                        SUM(t.status = 'completed'),
                        SUM(t.status = 'failed'),
                        SUM(t.status = 'skipped'),
                        AVG(t.duration_seconds),
                        MIN(t.duration_seconds),
                        MAX(t.duration_seconds),
                        MAX(t.started_at),
                        (SELECT t2.status FROM tasks t2
                         JOIN runs r2 ON t2.run_id = r2.id
                         WHERE t2.name = ?1 AND r2.project_id = p.id
                         ORDER BY t2.started_at DESC LIMIT 1)
                 FROM tasks t
                 JOIN runs r ON t.run_id = r.id
                 JOIN projects p ON r.project_id = p.id
                 WHERE t.name = ?1
                 GROUP BY p.id, p.hash, p.name
                 ORDER BY p.id",
            )?;

            let rows = stmt.query_map(params![task_name], |row| {
                let project_hash: String = row.get(1)?;
                let project_name: Option<String> = row.get(2)?;
                Ok(TaskStats {
                    project_id: row.get(0)?,
                    project_name: project_name.unwrap_or_else(|| project_hash.clone()),
                    project_hash,
                    task_name: task_name.to_string(),
                    total_executions: row.get::<_, i64>(3)? as u64,
                    successful_executions: row.get::<_, i64>(4)? as u64,
                    failed_executions: row.get::<_, i64>(5)? as u64,
                    skipped_executions: row.get::<_, i64>(6)? as u64,
                    avg_duration_seconds: row.get(7)?,
                    min_duration_seconds: row.get(8)?,
                    max_duration_seconds: row.get(9)?,
                    last_executed: row.get::<_, Option<i64>>(10)?.map(|t| t as u64),
                    last_status: Self::parse_last_status(row.get(11)?)?,
                })
            })?;

            rows.collect::<Result<Vec<_>, _>>()
                .context("Failed to fetch task stats")
        })
    }

    /// Get statistics for all tasks, ordered by execution count, grouped by project
    ///
    /// Same rewrite as `get_task_stats`: one `GROUP BY` query across every
    /// `(task, project)` pair instead of one `SELECT DISTINCT` plus eight
    /// queries per row. `LIMIT` binds `-1` for "no limit", SQLite's own
    /// spelling of it, rather than building two query strings.
    pub fn get_all_task_stats(&self, limit: Option<usize>) -> Result<Vec<TaskStats>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT t.name, p.id, p.hash, p.name,
                        COUNT(*),
                        SUM(t.status = 'completed'),
                        SUM(t.status = 'failed'),
                        SUM(t.status = 'skipped'),
                        AVG(t.duration_seconds),
                        MIN(t.duration_seconds),
                        MAX(t.duration_seconds),
                        MAX(t.started_at),
                        (SELECT t2.status FROM tasks t2
                         JOIN runs r2 ON t2.run_id = r2.id
                         WHERE t2.name = t.name AND r2.project_id = p.id
                         ORDER BY t2.started_at DESC LIMIT 1)
                 FROM tasks t
                 JOIN runs r ON t.run_id = r.id
                 JOIN projects p ON r.project_id = p.id
                 GROUP BY t.name, p.id, p.hash, p.name
                 ORDER BY COUNT(*) DESC, t.name ASC, p.id ASC
                 LIMIT ?1",
            )?;

            let limit = limit.map(|l| l as i64).unwrap_or(-1);
            let rows = stmt.query_map(params![limit], |row| {
                let task_name: String = row.get(0)?;
                let project_hash: String = row.get(2)?;
                let project_name: Option<String> = row.get(3)?;
                Ok(TaskStats {
                    project_id: row.get(1)?,
                    project_name: project_name.unwrap_or_else(|| project_hash.clone()),
                    project_hash,
                    task_name,
                    total_executions: row.get::<_, i64>(4)? as u64,
                    successful_executions: row.get::<_, i64>(5)? as u64,
                    failed_executions: row.get::<_, i64>(6)? as u64,
                    skipped_executions: row.get::<_, i64>(7)? as u64,
                    avg_duration_seconds: row.get(8)?,
                    min_duration_seconds: row.get(9)?,
                    max_duration_seconds: row.get(10)?,
                    last_executed: row.get::<_, Option<i64>>(11)?.map(|t| t as u64),
                    last_status: Self::parse_last_status(row.get(12)?)?,
                })
            })?;

            rows.collect::<Result<Vec<_>, _>>()
                .context("Failed to fetch task stats")
        })
    }

    pub fn get_runs_with_filters(
        &self,
        status_filter: Option<RunStatus>,
        project_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RunRecord>> {
        self.db.with_connection(|conn| {
            let mut query = format!("SELECT {RUN_COLUMNS} FROM runs r");

            let mut conditions = Vec::new();
            if project_filter.is_some() {
                query.push_str(" JOIN projects p ON r.project_id = p.id");
                conditions.push("p.hash = ?1".to_string());
            }
            if status_filter.is_some() {
                let param_num = if project_filter.is_some() { 2 } else { 1 };
                conditions.push(format!("r.status = ?{}", param_num));
            }

            if !conditions.is_empty() {
                query.push_str(" WHERE ");
                query.push_str(&conditions.join(" AND "));
            }

            query.push_str(" ORDER BY r.timestamp DESC LIMIT ?");
            let limit_param_num = 1 + project_filter.is_some() as usize + status_filter.is_some() as usize;
            query = query.replace("LIMIT ?", &format!("LIMIT ?{}", limit_param_num));

            let mut stmt = conn.prepare(&query)?;

            let rows = match (project_filter, status_filter) {
                (Some(project), Some(status)) => {
                    stmt.query_map(params![project, status.as_str(), limit as i64], Self::row_to_run_record)?
                }
                (Some(project), None) => stmt.query_map(params![project, limit as i64], Self::row_to_run_record)?,
                (None, Some(status)) => {
                    stmt.query_map(params![status.as_str(), limit as i64], Self::row_to_run_record)?
                }
                (None, None) => stmt.query_map(params![limit as i64], Self::row_to_run_record)?,
            };

            rows.collect::<Result<Vec<_>, _>>().context("Failed to fetch runs")
        })
    }

    pub fn find_old_runs(
        &self,
        keep_days: u64,
        keep_last: Option<usize>,
        keep_failed_days: Option<u64>,
        project_filter: Option<&str>,
    ) -> Result<Vec<RunRecord>> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("Failed to get current time")?
            .as_secs();

        let policy = Retention {
            keep_days,
            keep_last,
            keep_failed_days,
        };

        self.db.with_connection(|conn| {
            let mut query = format!("SELECT {RUN_COLUMNS} FROM runs r");

            if project_filter.is_some() {
                query.push_str(" JOIN projects p ON r.project_id = p.id WHERE p.hash = ?1");
            }

            query.push_str(" ORDER BY r.timestamp DESC");

            let mut stmt = conn.prepare(&query)?;
            let rows = if let Some(project) = project_filter {
                stmt.query_map(params![project], Self::row_to_run_record)?
            } else {
                stmt.query_map([], Self::row_to_run_record)?
            };

            let all_runs: Vec<RunRecord> = rows.collect::<Result<Vec<_>, _>>()?;

            // Retention lives in one pure function shared with the in-memory
            // store and the filesystem-scan fallback, because three private
            // copies is how `--keep-last` came to mean opposite things in two
            // of them.
            let ages: Vec<RunAge> = all_runs.iter().map(RunAge::from).collect();

            let mut runs_to_delete: Vec<RunRecord> = policy
                .expired(&ages, now)
                .into_iter()
                .map(|idx| all_runs[idx].clone())
                .collect();

            runs_to_delete.sort_by_key(|r| r.timestamp);

            Ok(runs_to_delete)
        })
    }

    /// Delete one run, by id, and optionally the directory it wrote into.
    ///
    /// Keyed on `id` rather than the timestamp so two runs from the same second
    /// are two runs.
    ///
    /// Ordering, which is the whole point of the split below: the directory path
    /// is resolved and fenced *before* `BEGIN`, the rows go in one immediate
    /// transaction, and `remove_dir_all` runs only after the commit. It used to
    /// commit first and fence afterwards, so a run directory that had been
    /// replaced by a symlink - the case the fence exists for - lost its rows and
    /// kept its directory, orphaned with nothing pointing at it. The unlink stays
    /// after the commit for the opposite reason: a failed unlink must not roll a
    /// committed delete back into existence.
    pub fn delete_run(&self, run_id: i64, delete_filesystem: bool, otto_home: &Path) -> Result<Option<RunRecord>> {
        log::debug!(
            "delete_run: run_id={run_id} delete_filesystem={delete_filesystem} otto_home={}",
            otto_home.display()
        );

        let Some(run_record) = self.read_run(run_id)? else {
            return Ok(None);
        };

        // Before BEGIN: a refusal here leaves the database untouched.
        let doomed_dir = if delete_filesystem {
            self.resolve_run_directory(&run_record, otto_home)?
        } else {
            None
        };

        let deleted = self.db.with_connection(|conn| {
            let tx = TransactionGuard::immediate(conn)?;

            // Re-read under the write lock: the row was read outside it, and a
            // concurrent otto may have deleted it in between. Deleting rows that
            // are already gone would still decrement `run_count`.
            let present: bool = conn.query_row("SELECT COUNT(*) FROM runs WHERE id = ?1", params![run_id], |row| {
                Ok(row.get::<_, i64>(0)? > 0)
            })?;
            if !present {
                return Ok(false);
            }

            // Explicit rather than relying on the CASCADE, which is only armed
            // when foreign keys are on.
            conn.execute("DELETE FROM tasks WHERE run_id = ?1", params![run_record.id])?;
            conn.execute("DELETE FROM runs WHERE id = ?1", params![run_record.id])?;

            // `run_count - 1` on its own went negative whenever a run was
            // deleted that the counter never counted.
            conn.execute(
                "UPDATE projects SET run_count = MAX(run_count - 1, 0) WHERE id = ?1",
                params![run_record.project_id],
            )?;

            tx.commit()?;
            Ok(true)
        })?;

        if !deleted {
            return Ok(None);
        }

        if let Some(dir) = doomed_dir {
            std::fs::remove_dir_all(&dir).context("Failed to delete run directory")?;
        }

        Ok(Some(run_record))
    }

    /// Read one run row.
    ///
    /// It used to read the project's hash and display name alongside it, for a
    /// run directory path derived from them. Nothing derives that path any more:
    /// a row that recorded no directory has its rows deleted and its directory,
    /// if there is one, reclaimed by path by `Clean`'s orphan sweep.
    fn read_run(&self, run_id: i64) -> Result<Option<RunRecord>> {
        self.db.with_connection(|conn| {
            let query = format!("SELECT {RUN_COLUMNS} FROM runs r WHERE r.id = ?1");
            Ok(conn
                .query_row(&query, params![run_id], Self::row_to_run_record)
                .optional()?)
        })
    }

    /// Resolve the directory a run wrote into and check it is safe to delete,
    /// returning the canonical path to remove, or `None` if there is nothing on
    /// disk.
    ///
    /// The path comes from the run row, which records it at run start. Cleanup
    /// used to rebuild `otto-<hash>` under a hardcoded `$HOME/.otto`, which
    /// matched neither the `<name>-<hash>` naming convention nor `OTTO_HOME`, so
    /// it deleted the rows and left every directory behind.
    ///
    /// Rows written before schema v5 carry no directory, and no path is derived
    /// for them. The derivation that used to be here could not be right: it
    /// rebuilt `<name>-<hash>/<timestamp>`, which cannot match the
    /// `<timestamp>-<seq>` directory a same-second run gets and cannot survive a
    /// moved or renamed ottofile, and `projects.hash` is a hash of the
    /// ottofile's *contents* (`parser.rs`) while the directory name carries a
    /// hash of the project's *path* (`workspace.rs`). A guess that misses
    /// deletes the row and orphans the directory, with the only pointer to it
    /// gone. `Clean` reclaims those by path instead, so the rows are all this
    /// has to remove.
    fn resolve_run_directory(&self, run: &RunRecord, otto_home: &Path) -> Result<Option<PathBuf>> {
        let Some(run_dir) = run.run_dir.clone() else {
            log::warn!(
                "Run {} recorded no directory; deleting its database rows only, and leaving any directory to the orphan sweep",
                run.timestamp
            );
            return Ok(None);
        };

        if std::fs::symlink_metadata(&run_dir).is_err() {
            // Reported, not skipped: a missing directory means the row and the
            // disk had already drifted apart, which is the thing worth knowing.
            log::warn!(
                "Run {} had no directory at {}; deleting its database rows only",
                run.timestamp,
                run_dir.display()
            );
            return Ok(None);
        }

        // Same fence as the filesystem-scan path in `clean`: never delete
        // through a symlink, never delete outside the otto root. The DB path had
        // no check at all, so a run directory replaced by a link deleted the
        // link's target instead.
        let canonical = crate::executor::pruning::ensure_deletable_under_root(&run_dir, otto_home)
            .context("Refusing to delete run directory")?;
        Ok(Some(canonical))
    }

    /// Ensure a project exists in the database, creating it if necessary
    /// Returns the project ID
    ///
    /// One upsert, not a SELECT followed by an INSERT: two otto runs starting
    /// together both saw no project and both inserted, and the loser failed on
    /// `hash`'s UNIQUE constraint, taking its whole run record with it.
    fn ensure_project(&self, conn: &rusqlite::Connection, hash: &str, ottofile_path: Option<&PathBuf>) -> Result<i64> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("Failed to get current time")?
            .as_secs();

        let name = crate::naming::project_name_from(ottofile_path.map(PathBuf::as_path), hash);

        conn.execute(
            "INSERT INTO projects (hash, name, ottofile_path, first_seen, last_seen, run_count)
                  VALUES (?1, ?2, ?3, ?4, ?4, 0)
             ON CONFLICT(hash) DO UPDATE SET
                 name = CASE WHEN excluded.ottofile_path IS NOT NULL THEN excluded.name ELSE projects.name END,
                 ottofile_path = COALESCE(excluded.ottofile_path, projects.ottofile_path),
                 last_seen = excluded.last_seen",
            params![
                hash,
                name,
                ottofile_path.map(|p| p.to_string_lossy().to_string()),
                now as i64,
            ],
        )?;

        // `last_insert_rowid` is only the new project's id on the INSERT branch,
        // so the id is read back by the key that is actually unique.
        let id: i64 = conn
            .query_row("SELECT id FROM projects WHERE hash = ?1", params![hash], |row| {
                row.get(0)
            })
            .context("Failed to read back the project id")?;

        Ok(id)
    }
}

/// Implement StateStore trait for StateManager
/// This allows StateManager to be used through the trait abstraction
impl StateStore for StateManager {
    fn record_run_start(&self, metadata: &RunMetadata) -> Result<i64> {
        StateManager::record_run_start(self, metadata)
    }

    fn record_run_complete(&self, run_id: i64, status: RunStatus, size_bytes: Option<u64>) -> Result<()> {
        StateManager::record_run_complete(self, run_id, status, size_bytes)
    }

    fn record_task_start(
        &self,
        run_id: i64,
        task_name: &str,
        script_hash: Option<&str>,
        stdout_path: Option<&PathBuf>,
        stderr_path: Option<&PathBuf>,
        script_path: Option<&PathBuf>,
    ) -> Result<i64> {
        StateManager::record_task_start(
            self,
            run_id,
            task_name,
            script_hash,
            stdout_path,
            stderr_path,
            script_path,
        )
    }

    fn record_task_complete(&self, task_id: i64, exit_code: i32, status: TaskStatus) -> Result<()> {
        StateManager::record_task_complete(self, task_id, exit_code, status)
    }

    fn record_task_skipped(
        &self,
        run_id: i64,
        task_name: &str,
        script_hash: Option<&str>,
        skip_reason: Option<&str>,
        skip_kind: Option<SkipKind>,
    ) -> Result<i64> {
        StateManager::record_task_skipped(self, run_id, task_name, script_hash, skip_reason, skip_kind)
    }

    fn get_task_history(&self, task_name: &str, limit: usize) -> Result<Vec<TaskRecord>> {
        StateManager::get_task_history(self, task_name, limit)
    }

    fn get_overall_stats(&self) -> Result<OverallStats> {
        StateManager::get_overall_stats(self)
    }

    fn get_all_projects(&self) -> Result<Vec<ProjectSummary>> {
        StateManager::get_all_projects(self)
    }

    fn get_task_stats(&self, task_name: &str) -> Result<Vec<TaskStats>> {
        StateManager::get_task_stats(self, task_name)
    }

    fn get_all_task_stats(&self, limit: Option<usize>) -> Result<Vec<TaskStats>> {
        StateManager::get_all_task_stats(self, limit)
    }

    fn get_runs_with_filters(
        &self,
        status_filter: Option<RunStatus>,
        project_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RunRecord>> {
        StateManager::get_runs_with_filters(self, status_filter, project_filter, limit)
    }

    fn find_old_runs(
        &self,
        keep_days: u64,
        keep_last: Option<usize>,
        keep_failed_days: Option<u64>,
        project_filter: Option<&str>,
    ) -> Result<Vec<RunRecord>> {
        StateManager::find_old_runs(self, keep_days, keep_last, keep_failed_days, project_filter)
    }

    fn delete_run(&self, run_id: i64, delete_filesystem: bool, otto_home: &Path) -> Result<Option<RunRecord>> {
        StateManager::delete_run(self, run_id, delete_filesystem, otto_home)
    }
}

#[path = "manager_tests.rs"]
mod tests;
