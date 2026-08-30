use eyre::{Context, Result};
use rusqlite::{OptionalExtension, params};
use std::path::PathBuf;
use std::time::SystemTime;

use super::db::DatabaseManager;
use super::metadata::RunMetadata;
use super::schema::{RunStatus, SkipKind, TaskStatus};
use crate::ports::StateStore;

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

    /// Try to create a StateManager, returning None if DB is unavailable
    /// This enables graceful degradation
    pub fn try_new() -> Option<Self> {
        Self::new().ok()
    }

    pub fn record_run_start(&self, metadata: &RunMetadata) -> Result<i64> {
        self.db.with_connection(|conn| {
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
                    project_id, timestamp, status, ottofile_path, cwd, user, hostname, args
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    project_id,
                    metadata.timestamp as i64,
                    RunStatus::Running.as_str(),
                    metadata.ottofile.as_ref().map(|p| p.to_string_lossy().to_string()),
                    metadata.cwd.as_ref().map(|p| p.to_string_lossy().to_string()),
                    metadata.user,
                    metadata.hostname,
                    args_json,
                ],
            )?;

            let run_id = conn.last_insert_rowid();

            conn.execute(
                "UPDATE projects
                 SET last_seen = ?1, run_count = run_count + 1
                 WHERE id = ?2",
                params![metadata.timestamp as i64, project_id],
            )?;

            Ok(run_id)
        })
    }

    /// Record the completion of a run
    pub fn record_run_complete(&self, timestamp: u64, status: RunStatus, size_bytes: Option<u64>) -> Result<()> {
        let ended_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("Failed to get current time")?
            .as_secs();

        let duration_seconds = ended_at.saturating_sub(timestamp) as f64;

        self.db.with_connection(|conn| {
            conn.execute(
                "UPDATE runs
                 SET status = ?1, duration_seconds = ?2, size_bytes = ?3, ended_at = ?4
                 WHERE timestamp = ?5",
                params![
                    status.as_str(),
                    duration_seconds,
                    size_bytes.map(|s| s as i64),
                    ended_at as i64,
                    timestamp as i64,
                ],
            )?;
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

    /// Record the completion of a task
    pub fn record_task_complete(&self, task_id: i64, exit_code: i32, status: TaskStatus) -> Result<()> {
        let ended_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("Failed to get current time")?
            .as_secs();

        self.db.with_connection(|conn| {
            let started_at: i64 =
                conn.query_row("SELECT started_at FROM tasks WHERE id = ?1", params![task_id], |row| {
                    row.get(0)
                })?;

            let duration_seconds = (ended_at as i64 - started_at) as f64;

            conn.execute(
                "UPDATE tasks
                 SET status = ?1, exit_code = ?2, ended_at = ?3, duration_seconds = ?4
                 WHERE id = ?5",
                params![status.as_str(), exit_code, ended_at as i64, duration_seconds, task_id],
            )?;

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
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO tasks (run_id, name, status, script_hash, skip_reason, skip_kind)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    run_id,
                    task_name,
                    TaskStatus::Skipped.as_str(),
                    script_hash,
                    skip_reason,
                    skip_kind.map(|k| k.as_str())
                ],
            )?;

            Ok(conn.last_insert_rowid())
        })
    }

    /// Get recent runs, optionally filtered by project hash
    pub fn get_recent_runs(&self, limit: usize, project_filter: Option<&str>) -> Result<Vec<RunRecord>> {
        self.db.with_connection(|conn| {
            let query = if let Some(_project_hash) = project_filter {
                "SELECT r.id, r.project_id, r.timestamp, r.status, r.duration_seconds,
                            r.size_bytes, r.ottofile_path, r.cwd, r.user, r.hostname, r.args, r.ended_at
                     FROM runs r
                     JOIN projects p ON r.project_id = p.id
                     WHERE p.hash = ?1
                     ORDER BY r.timestamp DESC
                     LIMIT ?2"
            } else {
                "SELECT r.id, r.project_id, r.timestamp, r.status, r.duration_seconds,
                            r.size_bytes, r.ottofile_path, r.cwd, r.user, r.hostname, r.args, r.ended_at
                     FROM runs r
                     ORDER BY r.timestamp DESC
                     LIMIT ?1"
            };

            let mut stmt = conn.prepare(query)?;

            let rows = if let Some(project_hash) = project_filter {
                stmt.query_map(params![project_hash, limit as i64], Self::row_to_run_record)?
            } else {
                stmt.query_map(params![limit as i64], Self::row_to_run_record)?
            };

            rows.collect::<Result<Vec<_>, _>>().context("Failed to fetch runs")
        })
    }

    pub fn get_run_tasks(&self, run_id: i64) -> Result<Vec<TaskRecord>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, run_id, name, status, script_hash, exit_code,
                        started_at, ended_at, duration_seconds,
                        stdout_path, stderr_path, script_path, skip_reason, skip_kind
                 FROM tasks
                 WHERE run_id = ?1
                 ORDER BY started_at ASC",
            )?;

            let rows = stmt.query_map(params![run_id], Self::row_to_task_record)?;

            rows.collect::<Result<Vec<_>, _>>().context("Failed to fetch tasks")
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
    fn row_to_run_record(row: &rusqlite::Row) -> rusqlite::Result<RunRecord> {
        let args_json: Option<String> = row.get(10)?;
        let args = args_json.and_then(|json| serde_json::from_str(&json).ok());

        let status_str: String = row.get(3)?;
        let status = RunStatus::parse(&status_str).unwrap_or(RunStatus::Failed);

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
        })
    }

    /// Helper to convert a database row to a TaskRecord
    fn row_to_task_record(row: &rusqlite::Row) -> rusqlite::Result<TaskRecord> {
        let status_str: String = row.get(3)?;
        let status = TaskStatus::parse(&status_str).unwrap_or(TaskStatus::Failed);

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
            skip_kind: row.get::<_, Option<String>>(13)?.as_deref().and_then(SkipKind::parse),
        })
    }

    /// Get overall system statistics
    pub fn get_overall_stats(&self) -> Result<OverallStats> {
        self.db.with_connection(|conn| {
            let total_runs: u64 = conn.query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))?;

            let successful_runs: u64 =
                conn.query_row("SELECT COUNT(*) FROM runs WHERE status = 'success'", [], |row| {
                    row.get(0)
                })?;

            let failed_runs: u64 = conn.query_row("SELECT COUNT(*) FROM runs WHERE status = 'failed'", [], |row| {
                row.get(0)
            })?;

            let running_runs: u64 =
                conn.query_row("SELECT COUNT(*) FROM runs WHERE status = 'running'", [], |row| {
                    row.get(0)
                })?;

            let total_tasks: u64 = conn.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))?;

            let total_disk_usage: i64 = conn
                .query_row("SELECT COALESCE(SUM(size_bytes), 0) FROM runs", [], |row| row.get(0))
                .unwrap_or(0);

            let total_duration_seconds: f64 = conn
                .query_row("SELECT COALESCE(SUM(duration_seconds), 0) FROM runs", [], |row| {
                    row.get(0)
                })
                .unwrap_or(0.0);

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
                    Ok(ProjectSummary {
                        id: row.get(0)?,
                        hash: row.get(1)?,
                        name: row
                            .get::<_, Option<String>>(2)?
                            .unwrap_or_else(|| row.get::<_, String>(1).unwrap()),
                        ottofile_path: row.get::<_, Option<String>>(3)?.map(PathBuf::from),
                        run_count: row.get::<_, i64>(4)? as u64,
                        last_seen: row.get::<_, i64>(5)? as u64,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(projects)
        })
    }

    /// Get statistics for a specific task across all projects
    pub fn get_task_stats(&self, task_name: &str) -> Result<Vec<TaskStats>> {
        self.db.with_connection(|conn| {
            // Get all projects that have this task
            let mut stmt = conn.prepare(
                "SELECT DISTINCT p.id, p.hash, p.name
                 FROM tasks t
                 JOIN runs r ON t.run_id = r.id
                 JOIN projects p ON r.project_id = p.id
                 WHERE t.name = ?1",
            )?;

            let projects: Vec<(i64, String, Option<String>)> = stmt
                .query_map(params![task_name], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;

            let mut stats = Vec::new();
            for (project_id, project_hash, project_name_opt) in projects {
                let project_name = project_name_opt.unwrap_or_else(|| project_hash.clone());

                let total_executions: u64 = conn.query_row(
                    "SELECT COUNT(*)
                     FROM tasks t
                     JOIN runs r ON t.run_id = r.id
                     WHERE t.name = ?1 AND r.project_id = ?2",
                    params![task_name, project_id],
                    |row| row.get(0),
                )?;

                let successful_executions: u64 = conn.query_row(
                    "SELECT COUNT(*)
                     FROM tasks t
                     JOIN runs r ON t.run_id = r.id
                     WHERE t.name = ?1 AND r.project_id = ?2 AND t.status = 'completed'",
                    params![task_name, project_id],
                    |row| row.get(0),
                )?;

                let failed_executions: u64 = conn.query_row(
                    "SELECT COUNT(*)
                     FROM tasks t
                     JOIN runs r ON t.run_id = r.id
                     WHERE t.name = ?1 AND r.project_id = ?2 AND t.status = 'failed'",
                    params![task_name, project_id],
                    |row| row.get(0),
                )?;

                let skipped_executions: u64 = conn.query_row(
                    "SELECT COUNT(*)
                     FROM tasks t
                     JOIN runs r ON t.run_id = r.id
                     WHERE t.name = ?1 AND r.project_id = ?2 AND t.status = 'skipped'",
                    params![task_name, project_id],
                    |row| row.get(0),
                )?;

                let avg_duration_seconds: Option<f64> = conn
                    .query_row(
                        "SELECT AVG(t.duration_seconds)
                         FROM tasks t
                         JOIN runs r ON t.run_id = r.id
                         WHERE t.name = ?1 AND r.project_id = ?2 AND t.duration_seconds IS NOT NULL",
                        params![task_name, project_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();

                let min_duration_seconds: Option<f64> = conn
                    .query_row(
                        "SELECT MIN(t.duration_seconds)
                         FROM tasks t
                         JOIN runs r ON t.run_id = r.id
                         WHERE t.name = ?1 AND r.project_id = ?2 AND t.duration_seconds IS NOT NULL",
                        params![task_name, project_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();

                let max_duration_seconds: Option<f64> = conn
                    .query_row(
                        "SELECT MAX(t.duration_seconds)
                         FROM tasks t
                         JOIN runs r ON t.run_id = r.id
                         WHERE t.name = ?1 AND r.project_id = ?2 AND t.duration_seconds IS NOT NULL",
                        params![task_name, project_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();

                let (last_executed, last_status_str): (Option<i64>, Option<String>) = conn
                    .query_row(
                        "SELECT t.started_at, t.status
                         FROM tasks t
                         JOIN runs r ON t.run_id = r.id
                         WHERE t.name = ?1 AND r.project_id = ?2
                         ORDER BY t.started_at DESC LIMIT 1",
                        params![task_name, project_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?
                    .unwrap_or((None, None));

                let last_status = last_status_str.and_then(|s| TaskStatus::parse(&s));

                stats.push(TaskStats {
                    project_id,
                    project_hash,
                    project_name,
                    task_name: task_name.to_string(),
                    total_executions,
                    successful_executions,
                    failed_executions,
                    skipped_executions,
                    avg_duration_seconds,
                    min_duration_seconds,
                    max_duration_seconds,
                    last_executed: last_executed.map(|t| t as u64),
                    last_status,
                });
            }

            Ok(stats)
        })
    }

    /// Get statistics for all tasks, ordered by execution count, grouped by project
    pub fn get_all_task_stats(&self, limit: Option<usize>) -> Result<Vec<TaskStats>> {
        self.db.with_connection(|conn| {
            let query = if let Some(limit) = limit {
                format!(
                    "SELECT DISTINCT t.name, p.id, p.hash, p.name
                     FROM tasks t
                     JOIN runs r ON t.run_id = r.id
                     JOIN projects p ON r.project_id = p.id
                     ORDER BY (
                         SELECT COUNT(*)
                         FROM tasks t2
                         JOIN runs r2 ON t2.run_id = r2.id
                         WHERE t2.name = t.name AND r2.project_id = p.id
                     ) DESC
                     LIMIT {}",
                    limit
                )
            } else {
                "SELECT DISTINCT t.name, p.id, p.hash, p.name
                 FROM tasks t
                 JOIN runs r ON t.run_id = r.id
                 JOIN projects p ON r.project_id = p.id
                 ORDER BY (
                     SELECT COUNT(*)
                     FROM tasks t2
                     JOIN runs r2 ON t2.run_id = r2.id
                     WHERE t2.name = t.name AND r2.project_id = p.id
                 ) DESC"
                    .to_string()
            };

            let mut stmt = conn.prepare(&query)?;
            let task_projects: Vec<(String, i64, String, String)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
                .collect::<Result<Vec<_>, _>>()?;

            let mut stats = Vec::new();
            for (task_name, project_id, project_hash, project_name) in task_projects {
                // Calculate stats for this task within this project
                let total_executions: u64 = conn.query_row(
                    "SELECT COUNT(*)
                     FROM tasks t
                     JOIN runs r ON t.run_id = r.id
                     WHERE t.name = ?1 AND r.project_id = ?2",
                    params![&task_name, project_id],
                    |row| row.get(0),
                )?;

                if total_executions == 0 {
                    continue;
                }

                let successful_executions: u64 = conn.query_row(
                    "SELECT COUNT(*)
                     FROM tasks t
                     JOIN runs r ON t.run_id = r.id
                     WHERE t.name = ?1 AND r.project_id = ?2 AND t.status = 'completed'",
                    params![&task_name, project_id],
                    |row| row.get(0),
                )?;

                let failed_executions: u64 = conn.query_row(
                    "SELECT COUNT(*)
                     FROM tasks t
                     JOIN runs r ON t.run_id = r.id
                     WHERE t.name = ?1 AND r.project_id = ?2 AND t.status = 'failed'",
                    params![&task_name, project_id],
                    |row| row.get(0),
                )?;

                let skipped_executions: u64 = conn.query_row(
                    "SELECT COUNT(*)
                     FROM tasks t
                     JOIN runs r ON t.run_id = r.id
                     WHERE t.name = ?1 AND r.project_id = ?2 AND t.status = 'skipped'",
                    params![&task_name, project_id],
                    |row| row.get(0),
                )?;

                let avg_duration_seconds: Option<f64> = conn
                    .query_row(
                        "SELECT AVG(t.duration_seconds)
                         FROM tasks t
                         JOIN runs r ON t.run_id = r.id
                         WHERE t.name = ?1 AND r.project_id = ?2 AND t.duration_seconds IS NOT NULL",
                        params![&task_name, project_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();

                let min_duration_seconds: Option<f64> = conn
                    .query_row(
                        "SELECT MIN(t.duration_seconds)
                         FROM tasks t
                         JOIN runs r ON t.run_id = r.id
                         WHERE t.name = ?1 AND r.project_id = ?2 AND t.duration_seconds IS NOT NULL",
                        params![&task_name, project_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();

                let max_duration_seconds: Option<f64> = conn
                    .query_row(
                        "SELECT MAX(t.duration_seconds)
                         FROM tasks t
                         JOIN runs r ON t.run_id = r.id
                         WHERE t.name = ?1 AND r.project_id = ?2 AND t.duration_seconds IS NOT NULL",
                        params![&task_name, project_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();

                let (last_executed, last_status_str): (Option<i64>, Option<String>) = conn
                    .query_row(
                        "SELECT t.started_at, t.status
                         FROM tasks t
                         JOIN runs r ON t.run_id = r.id
                         WHERE t.name = ?1 AND r.project_id = ?2
                         ORDER BY t.started_at DESC
                         LIMIT 1",
                        params![&task_name, project_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?
                    .unwrap_or((None, None));

                let last_status = last_status_str.and_then(|s| TaskStatus::parse(&s));

                stats.push(TaskStats {
                    project_id,
                    project_hash: project_hash.clone(),
                    project_name: project_name.clone(),
                    task_name: task_name.clone(),
                    total_executions,
                    successful_executions,
                    failed_executions,
                    skipped_executions,
                    avg_duration_seconds,
                    min_duration_seconds,
                    max_duration_seconds,
                    last_executed: last_executed.map(|t| t as u64),
                    last_status,
                });
            }

            Ok(stats)
        })
    }

    pub fn get_runs_with_filters(
        &self,
        status_filter: Option<RunStatus>,
        project_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RunRecord>> {
        self.db.with_connection(|conn| {
            let mut query = String::from(
                "SELECT r.id, r.project_id, r.timestamp, r.status, r.duration_seconds,
                        r.size_bytes, r.ottofile_path, r.cwd, r.user, r.hostname, r.args, r.ended_at
                 FROM runs r",
            );

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
        let cutoff_timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("Failed to get current time")?
            .as_secs()
            .saturating_sub(keep_days * 24 * 60 * 60);

        let failed_cutoff_timestamp = keep_failed_days.map(|days| {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .saturating_sub(days * 24 * 60 * 60)
        });

        self.db.with_connection(|conn| {
            let mut query = String::from(
                "SELECT r.id, r.project_id, r.timestamp, r.status, r.duration_seconds,
                        r.size_bytes, r.ottofile_path, r.cwd, r.user, r.hostname, r.args, r.ended_at
                 FROM runs r",
            );

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

            // Apply retention logic
            let mut runs_to_delete = Vec::new();
            let keep_count = keep_last.unwrap_or(0);

            for (idx, run) in all_runs.iter().enumerate() {
                // Always keep the N most recent runs if --keep-last is specified
                if idx < keep_count {
                    continue;
                }

                // Apply different cutoff for failed runs if specified
                let cutoff = if matches!(run.status, RunStatus::Failed) {
                    failed_cutoff_timestamp.unwrap_or(cutoff_timestamp)
                } else {
                    cutoff_timestamp
                };

                if run.timestamp < cutoff {
                    runs_to_delete.push(run.clone());
                }
            }

            runs_to_delete.sort_by_key(|r| r.timestamp);

            Ok(runs_to_delete)
        })
    }

    pub fn delete_run(&self, timestamp: u64, delete_filesystem: bool) -> Result<Option<RunRecord>> {
        let run = self.db.with_connection(|conn| {
            let run: Option<RunRecord> = conn
                .query_row(
                    "SELECT r.id, r.project_id, r.timestamp, r.status, r.duration_seconds,
                            r.size_bytes, r.ottofile_path, r.cwd, r.user, r.hostname, r.args, r.ended_at
                     FROM runs r
                     WHERE r.timestamp = ?1",
                    params![timestamp as i64],
                    Self::row_to_run_record,
                )
                .optional()?;

            if let Some(ref run_record) = run {
                // Delete all tasks for this run (CASCADE will handle this, but explicit is clearer)
                conn.execute("DELETE FROM tasks WHERE run_id = ?1", params![run_record.id])?;

                conn.execute("DELETE FROM runs WHERE id = ?1", params![run_record.id])?;

                conn.execute(
                    "UPDATE projects SET run_count = run_count - 1 WHERE id = ?1",
                    params![run_record.project_id],
                )?;
            }

            Ok(run)
        })?;

        // Optionally delete filesystem directory
        if delete_filesystem && let Some(ref run_record) = run {
            // Construct the path: ~/.otto/otto-<hash>/<timestamp>/
            if let Some(home) = dirs::home_dir() {
                // We need the project hash - query it
                let project_hash = self.db.with_connection(|conn| {
                    let hash = conn.query_row(
                        "SELECT hash FROM projects WHERE id = ?1",
                        params![run_record.project_id],
                        |row| row.get::<_, String>(0),
                    )?;
                    Ok(hash)
                })?;

                let run_dir = home
                    .join(".otto")
                    .join(format!("otto-{}", project_hash))
                    .join(timestamp.to_string());

                if run_dir.exists() {
                    std::fs::remove_dir_all(&run_dir).context("Failed to delete run directory")?;
                }
            }
        }

        Ok(run)
    }

    /// Ensure a project exists in the database, creating it if necessary
    /// Returns the project ID
    fn ensure_project(&self, conn: &rusqlite::Connection, hash: &str, ottofile_path: Option<&PathBuf>) -> Result<i64> {
        // Try to find existing project
        let existing: Option<i64> = conn
            .query_row("SELECT id FROM projects WHERE hash = ?1", params![hash], |row| {
                row.get(0)
            })
            .optional()
            .context("Failed to query for existing project")?;

        if let Some(id) = existing {
            return Ok(id);
        }

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("Failed to get current time")?
            .as_secs();

        // Extract project name from ottofile path, or use hash as fallback
        let name = if let Some(path) = ottofile_path {
            std::path::Path::new(&path)
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or(hash)
                .to_string()
        } else {
            hash.to_string()
        };

        conn.execute(
            "INSERT INTO projects (hash, name, ottofile_path, first_seen, last_seen, run_count)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            params![
                hash,
                name,
                ottofile_path.map(|p| p.to_string_lossy().to_string()),
                now as i64,
                now as i64,
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }
}

/// Implement StateStore trait for StateManager
/// This allows StateManager to be used through the trait abstraction
impl StateStore for StateManager {
    fn record_run_start(&self, metadata: &RunMetadata) -> Result<i64> {
        StateManager::record_run_start(self, metadata)
    }

    fn record_run_complete(&self, timestamp: u64, status: RunStatus, size_bytes: Option<u64>) -> Result<()> {
        StateManager::record_run_complete(self, timestamp, status, size_bytes)
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

    fn get_recent_runs(&self, limit: usize, project_filter: Option<&str>) -> Result<Vec<RunRecord>> {
        StateManager::get_recent_runs(self, limit, project_filter)
    }

    fn get_run_tasks(&self, run_id: i64) -> Result<Vec<TaskRecord>> {
        StateManager::get_run_tasks(self, run_id)
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

    fn delete_run(&self, timestamp: u64, delete_filesystem: bool) -> Result<Option<RunRecord>> {
        StateManager::delete_run(self, timestamp, delete_filesystem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_test_manager() -> Result<(StateManager, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let manager = StateManager::with_db_path(db_path)?;
        Ok((manager, temp_dir))
    }

    #[test]
    fn test_record_run_start() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);

        let run_id = manager.record_run_start(&metadata)?;
        assert!(run_id > 0);

        Ok(())
    }

    #[test]
    fn test_record_run_complete() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);

        manager.record_run_start(&metadata)?;
        manager.record_run_complete(1234567890, RunStatus::Success, Some(1024))?;

        let runs = manager.get_recent_runs(1, None)?;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, RunStatus::Success);
        assert_eq!(runs[0].size_bytes, Some(1024));
        assert!(runs[0].duration_seconds.is_some());

        Ok(())
    }

    #[test]
    fn test_get_recent_runs() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        for i in 0..5 {
            let metadata = RunMetadata::minimal(
                Some(PathBuf::from("/test/otto.yml")),
                "abc123".to_string(),
                1234567890 + i,
            );
            manager.record_run_start(&metadata)?;
        }

        let runs = manager.get_recent_runs(3, None)?;
        assert_eq!(runs.len(), 3);

        assert!(runs[0].timestamp > runs[1].timestamp);
        assert!(runs[1].timestamp > runs[2].timestamp);

        Ok(())
    }

    #[test]
    fn test_get_recent_runs_with_project_filter() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        for i in 0..3 {
            let metadata1 = RunMetadata::minimal(
                Some(PathBuf::from("/test/otto.yml")),
                "abc123".to_string(),
                1234567890 + i,
            );
            manager.record_run_start(&metadata1)?;

            let metadata2 = RunMetadata::minimal(
                Some(PathBuf::from("/test/otto.yml")),
                "def456".to_string(),
                1234567890 + i + 100,
            );
            manager.record_run_start(&metadata2)?;
        }

        let runs = manager.get_recent_runs(10, Some("abc123"))?;
        assert_eq!(runs.len(), 3);

        // All runs should be for abc123
        // We can verify by checking timestamps match what we inserted
        assert!(runs.iter().all(|r| r.timestamp < 1234567890 + 100));

        Ok(())
    }

    #[test]
    fn test_ensure_project_creates_new() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        manager.db.with_connection(|conn| {
            let project_id1 = manager.ensure_project(conn, "test123", Some(&PathBuf::from("/test/otto.yml")))?;
            assert!(project_id1 > 0);

            // Calling again should return same ID
            let project_id2 = manager.ensure_project(conn, "test123", Some(&PathBuf::from("/test/otto.yml")))?;
            assert_eq!(project_id1, project_id2);

            Ok(())
        })
    }

    #[test]
    fn test_full_metadata_recording() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let metadata = RunMetadata::full(
            Some(PathBuf::from("/test/otto.yml")),
            "abc123".to_string(),
            1234567890,
            Some(PathBuf::from("/home/user/project")),
            Some("testuser".to_string()),
            Some("testhost".to_string()),
            Some(vec!["build".to_string(), "test".to_string()]),
        );

        manager.record_run_start(&metadata)?;

        let runs = manager.get_recent_runs(1, None)?;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].cwd, Some(PathBuf::from("/home/user/project")));
        assert_eq!(runs[0].user, Some("testuser".to_string()));
        assert_eq!(runs[0].hostname, Some("testhost".to_string()));
        assert_eq!(runs[0].args, Some(vec!["build".to_string(), "test".to_string()]));

        Ok(())
    }

    #[test]
    fn test_try_new_graceful_failure() {
        // This test verifies that try_new() returns None for invalid paths
        // We can't easily test this without mocking, but we can at least verify it compiles
        let _result = StateManager::try_new();
    }

    #[test]
    fn test_record_task_start() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
        let run_id = manager.record_run_start(&metadata)?;

        let task_id = manager.record_task_start(
            run_id,
            "test-task",
            Some("hash123"),
            Some(&PathBuf::from("/tmp/stdout.log")),
            Some(&PathBuf::from("/tmp/stderr.log")),
            Some(&PathBuf::from("/tmp/script.sh")),
        )?;

        assert!(task_id > 0);

        let tasks = manager.get_run_tasks(run_id)?;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "test-task");
        assert_eq!(tasks[0].status, TaskStatus::Running);
        assert_eq!(tasks[0].script_hash, Some("hash123".to_string()));

        Ok(())
    }

    #[test]
    fn test_record_task_complete() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
        let run_id = manager.record_run_start(&metadata)?;

        let task_id = manager.record_task_start(run_id, "test-task", None, None, None, None)?;
        manager.record_task_complete(task_id, 0, TaskStatus::Completed)?;

        let tasks = manager.get_run_tasks(run_id)?;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, TaskStatus::Completed);
        assert_eq!(tasks[0].exit_code, Some(0));
        assert!(tasks[0].ended_at.is_some());
        assert!(tasks[0].duration_seconds.is_some());

        Ok(())
    }

    #[test]
    fn test_record_task_failed() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
        let run_id = manager.record_run_start(&metadata)?;

        let task_id = manager.record_task_start(run_id, "test-task", None, None, None, None)?;
        manager.record_task_complete(task_id, 1, TaskStatus::Failed)?;

        let tasks = manager.get_run_tasks(run_id)?;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, TaskStatus::Failed);
        assert_eq!(tasks[0].exit_code, Some(1));

        Ok(())
    }

    #[test]
    fn test_record_task_skipped() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
        let run_id = manager.record_run_start(&metadata)?;

        let task_id = manager.record_task_skipped(
            run_id,
            "test-task",
            Some("hash123"),
            Some("dep build failed; this task required when: success"),
            Some(SkipKind::Unreachable),
        )?;
        assert!(task_id > 0);

        let tasks = manager.get_run_tasks(run_id)?;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "test-task");
        assert_eq!(tasks[0].status, TaskStatus::Skipped);
        assert_eq!(tasks[0].script_hash, Some("hash123".to_string()));
        assert_eq!(
            tasks[0].skip_reason.as_deref(),
            Some("dep build failed; this task required when: success")
        );
        assert_eq!(
            tasks[0].skip_kind,
            Some(SkipKind::Unreachable),
            "the typed kind round-trips through the tasks.skip_kind column"
        );

        Ok(())
    }

    #[test]
    fn test_get_run_tasks() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
        let run_id = manager.record_run_start(&metadata)?;

        let task_id1 = manager.record_task_start(run_id, "task-1", None, None, None, None)?;
        let task_id2 = manager.record_task_start(run_id, "task-2", None, None, None, None)?;
        let task_id3 = manager.record_task_start(run_id, "task-3", None, None, None, None)?;

        manager.record_task_complete(task_id1, 0, TaskStatus::Completed)?;
        manager.record_task_complete(task_id2, 1, TaskStatus::Failed)?;
        manager.record_task_complete(task_id3, 0, TaskStatus::Completed)?;

        let tasks = manager.get_run_tasks(run_id)?;
        assert_eq!(tasks.len(), 3);

        // Tasks should be ordered by started_at
        assert_eq!(tasks[0].name, "task-1");
        assert_eq!(tasks[1].name, "task-2");
        assert_eq!(tasks[2].name, "task-3");

        Ok(())
    }

    #[test]
    fn test_get_task_history() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        for i in 0..5 {
            let metadata = RunMetadata::minimal(
                Some(PathBuf::from("/test/otto.yml")),
                "abc123".to_string(),
                1234567890 + i,
            );
            let run_id = manager.record_run_start(&metadata)?;

            let task_id = manager.record_task_start(run_id, "build", None, None, None, None)?;
            manager.record_task_complete(task_id, 0, TaskStatus::Completed)?;

            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let history = manager.get_task_history("build", 3)?;
        assert_eq!(history.len(), 3);

        // Should be ordered by started_at descending (newest first)
        // Use >= instead of > since timestamps might be the same in fast execution
        assert!(history[0].started_at >= history[1].started_at);
        assert!(history[1].started_at >= history[2].started_at);

        // All should be the same task name
        assert!(history.iter().all(|t| t.name == "build"));

        Ok(())
    }

    #[test]
    fn test_task_with_all_fields() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
        let run_id = manager.record_run_start(&metadata)?;

        let task_id = manager.record_task_start(
            run_id,
            "complex-task",
            Some("script_hash_123"),
            Some(&PathBuf::from("/tmp/stdout.log")),
            Some(&PathBuf::from("/tmp/stderr.log")),
            Some(&PathBuf::from("/tmp/script.sh")),
        )?;

        manager.record_task_complete(task_id, 0, TaskStatus::Completed)?;

        let tasks = manager.get_run_tasks(run_id)?;
        assert_eq!(tasks.len(), 1);

        let task = &tasks[0];
        assert_eq!(task.name, "complex-task");
        assert_eq!(task.script_hash, Some("script_hash_123".to_string()));
        assert_eq!(task.stdout_path, Some(PathBuf::from("/tmp/stdout.log")));
        assert_eq!(task.stderr_path, Some(PathBuf::from("/tmp/stderr.log")));
        assert_eq!(task.script_path, Some(PathBuf::from("/tmp/script.sh")));
        assert_eq!(task.exit_code, Some(0));
        assert!(task.started_at.is_some());
        assert!(task.ended_at.is_some());
        assert!(task.duration_seconds.is_some());

        Ok(())
    }

    #[test]
    fn test_find_old_runs_basic() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

        let old_timestamp = now - (40 * 24 * 60 * 60); // 40 days old
        let recent_timestamp = now - (10 * 24 * 60 * 60); // 10 days old

        let metadata1 = RunMetadata::minimal(
            Some(PathBuf::from("/test/otto.yml")),
            "abc123".to_string(),
            old_timestamp,
        );
        manager.record_run_start(&metadata1)?;
        manager.record_run_complete(old_timestamp, RunStatus::Success, Some(1024))?;

        let metadata2 = RunMetadata::minimal(
            Some(PathBuf::from("/test/otto.yml")),
            "abc123".to_string(),
            recent_timestamp,
        );
        manager.record_run_start(&metadata2)?;
        manager.record_run_complete(recent_timestamp, RunStatus::Success, Some(2048))?;

        // Find runs older than 30 days
        let old_runs = manager.find_old_runs(30, None, None, None)?;

        assert_eq!(old_runs.len(), 1);
        assert_eq!(old_runs[0].timestamp, old_timestamp);

        Ok(())
    }

    #[test]
    fn test_find_old_runs_with_keep_last() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

        for i in 0..5 {
            let timestamp = now - ((40 + i) * 24 * 60 * 60);
            let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), timestamp);
            manager.record_run_start(&metadata)?;
            manager.record_run_complete(timestamp, RunStatus::Success, Some(1024))?;
        }

        // Find old runs but keep the 2 most recent
        let old_runs = manager.find_old_runs(30, Some(2), None, None)?;

        // Should only return 3 runs (5 - 2 kept)
        assert_eq!(old_runs.len(), 3);

        // The oldest runs should be returned
        assert!(old_runs[0].timestamp < old_runs[1].timestamp);
        assert!(old_runs[1].timestamp < old_runs[2].timestamp);

        Ok(())
    }

    #[test]
    fn test_find_old_runs_with_keep_failed() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

        let success_timestamp = now - (40 * 24 * 60 * 60);
        let metadata1 = RunMetadata::minimal(
            Some(PathBuf::from("/test/otto.yml")),
            "abc123".to_string(),
            success_timestamp,
        );
        manager.record_run_start(&metadata1)?;
        manager.record_run_complete(success_timestamp, RunStatus::Success, Some(1024))?;

        let failed_timestamp = now - (39 * 24 * 60 * 60);
        let metadata2 = RunMetadata::minimal(
            Some(PathBuf::from("/test/otto.yml")),
            "abc123".to_string(),
            failed_timestamp,
        );
        manager.record_run_start(&metadata2)?;
        manager.record_run_complete(failed_timestamp, RunStatus::Failed, Some(2048))?;

        // Find runs older than 30 days, but keep failed runs for 45 days
        let old_runs = manager.find_old_runs(30, None, Some(45), None)?;

        // Should only return the successful run (failed run kept longer)
        assert_eq!(old_runs.len(), 1);
        assert_eq!(old_runs[0].timestamp, success_timestamp);
        assert_eq!(old_runs[0].status, RunStatus::Success);

        Ok(())
    }

    #[test]
    fn test_find_old_runs_with_project_filter() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
        let old_timestamp = now - (40 * 24 * 60 * 60);

        let metadata1 = RunMetadata::minimal(
            Some(PathBuf::from("/test/otto.yml")),
            "abc123".to_string(),
            old_timestamp,
        );
        manager.record_run_start(&metadata1)?;
        manager.record_run_complete(old_timestamp, RunStatus::Success, Some(1024))?;

        let metadata2 = RunMetadata::minimal(
            Some(PathBuf::from("/test/otto2.yml")),
            "def456".to_string(),
            old_timestamp + 1,
        );
        manager.record_run_start(&metadata2)?;
        manager.record_run_complete(old_timestamp + 1, RunStatus::Success, Some(2048))?;

        // Find old runs for specific project
        let old_runs = manager.find_old_runs(30, None, None, Some("abc123"))?;

        assert_eq!(old_runs.len(), 1);
        assert_eq!(old_runs[0].timestamp, old_timestamp);

        Ok(())
    }

    #[test]
    fn test_delete_run_database_only() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
        manager.record_run_start(&metadata)?;
        manager.record_run_complete(1234567890, RunStatus::Success, Some(1024))?;

        let runs_before = manager.get_recent_runs(10, None)?;
        assert_eq!(runs_before.len(), 1);

        let deleted = manager.delete_run(1234567890, false)?;
        assert!(deleted.is_some());
        assert_eq!(deleted.unwrap().timestamp, 1234567890);

        let runs_after = manager.get_recent_runs(10, None)?;
        assert_eq!(runs_after.len(), 0);

        Ok(())
    }

    #[test]
    fn test_delete_run_with_tasks() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
        let run_id = manager.record_run_start(&metadata)?;

        let task_id1 = manager.record_task_start(run_id, "task1", None, None, None, None)?;
        manager.record_task_complete(task_id1, 0, TaskStatus::Completed)?;

        let task_id2 = manager.record_task_start(run_id, "task2", None, None, None, None)?;
        manager.record_task_complete(task_id2, 1, TaskStatus::Failed)?;

        let tasks_before = manager.get_run_tasks(run_id)?;
        assert_eq!(tasks_before.len(), 2);

        manager.delete_run(1234567890, false)?;

        let tasks_after = manager.get_run_tasks(run_id)?;
        assert_eq!(tasks_after.len(), 0);

        Ok(())
    }

    #[test]
    fn test_delete_run_updates_project_count() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        for i in 0..3 {
            let metadata = RunMetadata::minimal(
                Some(PathBuf::from("/test/otto.yml")),
                "abc123".to_string(),
                1234567890 + i,
            );
            manager.record_run_start(&metadata)?;
        }

        manager.delete_run(1234567891, false)?;

        let runs = manager.get_recent_runs(10, Some("abc123"))?;
        assert_eq!(runs.len(), 2);

        Ok(())
    }

    #[test]
    fn test_delete_nonexistent_run() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        // Try to delete a run that doesn't exist
        let deleted = manager.delete_run(9999999999, false)?;
        assert!(deleted.is_none());

        Ok(())
    }

    #[test]
    fn test_find_old_runs_empty_database() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        // Find old runs in empty database
        let old_runs = manager.find_old_runs(30, None, None, None)?;
        assert_eq!(old_runs.len(), 0);

        Ok(())
    }

    #[test]
    fn test_find_old_runs_all_recent() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
        let recent_timestamp = now - (5 * 24 * 60 * 60); // 5 days old

        let metadata = RunMetadata::minimal(
            Some(PathBuf::from("/test/otto.yml")),
            "abc123".to_string(),
            recent_timestamp,
        );
        manager.record_run_start(&metadata)?;
        manager.record_run_complete(recent_timestamp, RunStatus::Success, Some(1024))?;

        // Find runs older than 30 days (should find nothing)
        let old_runs = manager.find_old_runs(30, None, None, None)?;
        assert_eq!(old_runs.len(), 0);

        Ok(())
    }

    #[test]
    fn test_find_old_runs_complex_policy() -> Result<()> {
        let (manager, _temp_dir) = create_test_manager()?;

        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

        for i in 0..10 {
            let timestamp = now - ((40 + i) * 24 * 60 * 60);
            let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), timestamp);
            manager.record_run_start(&metadata)?;
            let status = if i % 2 == 0 { RunStatus::Success } else { RunStatus::Failed };
            manager.record_run_complete(timestamp, status, Some(1024))?;
        }

        // Keep 3 most recent, delete successful runs older than 30 days, keep failed runs for 50 days
        let old_runs = manager.find_old_runs(30, Some(3), Some(50), None)?;

        // Should get 7 runs total (10 - 3 kept)
        // But failed runs are kept for 50 days, so all failed runs in the deletable set should be excluded
        assert!(old_runs.len() <= 7);

        // All returned runs should be either:
        // 1. Successful runs older than 30 days (not in the keep_last 3)
        // 2. No failed runs should be in the list (they're kept for 50 days)
        for run in &old_runs {
            if run.status == RunStatus::Failed {
                // Failed runs older than 50 days
                let age_days = (now - run.timestamp) / (24 * 60 * 60);
                assert!(age_days > 50, "Failed run should only be deleted if older than 50 days");
            }
        }

        Ok(())
    }
}
