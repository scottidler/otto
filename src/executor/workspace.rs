use crate::executor::layout::{directory_size, expand_tilde, resolve_otto_home, run_dir_name, run_root};
use crate::ports::{FileSystem, RealFs, StateStore, record_blocking};
use eyre::{Result, eyre};
use log::warn;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use yaml_serde;

use super::state::{RunMetadata, StateManager};

/// How many same-second run directories to disambiguate before giving up. Far
/// past any realistic burst; the bound exists so a pathological filesystem cannot
/// spin here forever.
const RUN_DIR_ATTEMPTS: u32 = 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    pub prog: String,
    pub cwd: PathBuf,
    pub user: String,
    pub timestamp: u64,
    pub hash: String,
    pub ottofile: Option<PathBuf>,
    pub args: Vec<String>,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self::new()
    }
}

/// The process's current directory, or an error that says which operation
/// failed and why it plausibly failed.
///
/// Every production caller used to go through a bare `env::current_dir()?`.
/// `io::Error`'s Display is just `No such file or directory (os error 2)`: no
/// operation, no path, nothing a user can act on. Running otto from a deleted
/// directory printed exactly that and exited 1. The one place that did say
/// something - the `warn!` in `ExecutionContext::new` below - was unreachable,
/// because every one of those bare calls runs first and aborts.
///
/// Routing them all through here makes the failure observable twice: a `warn!`
/// naming the operation for anyone with logging on, and an error message that
/// names it for everyone else.
pub fn current_dir() -> Result<PathBuf> {
    std::env::current_dir().map_err(|e| {
        warn!("current_dir() failed ({e}); the current directory may have been deleted or become unreadable");
        eyre!("cannot determine the current directory (it may have been deleted, or be unreadable): {e}")
    })
}

impl ExecutionContext {
    pub fn new() -> Self {
        // Same warn, same wording, via the shared helper - this fallback is the
        // one caller that substitutes a default instead of propagating.
        let cwd = current_dir().unwrap_or_else(|_| {
            warn!("falling back to \"/\" as the current directory");
            PathBuf::from("/")
        });
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_else(|e| {
                warn!("system clock is before the Unix epoch ({e}), recording timestamp 0");
                std::time::Duration::default()
            })
            .as_secs();
        Self {
            prog: "otto".to_string(),
            cwd,
            user: std::env::var("USER").unwrap_or_else(|_| "unknown".to_string()),
            timestamp,
            hash: "test".to_string(),
            ottofile: None,
            args: vec!["otto".to_string()],
        }
    }
}

/// Handles Otto's directory structure and storage paths
pub struct Workspace<F: FileSystem = RealFs> {
    // Base paths
    home: PathBuf, // ~/.otto
    root: PathBuf, // Current project directory
    hash: String,  // First 8 chars of project path hash
    time: u64,     // Current run timestamp

    // Computed paths
    project: PathBuf, // <name>-<hash>
    cache: PathBuf,   // <name>-<hash>/.cache
    run: PathBuf,     // <name>-<hash>/<timestamp>

    // Database integration
    db_run_id: std::sync::Mutex<Option<i64>>, // Run ID from database
    state_store: Option<Arc<dyn StateStore>>, // Optional state store for DB operations

    // Filesystem abstraction
    fs: Arc<F>,
}

impl Workspace {
    /// Create a new Workspace with the default RealFs filesystem
    pub async fn new(root: PathBuf) -> Result<Self> {
        Self::new_with_fs(root, Arc::new(RealFs)).await
    }
}

impl<F: FileSystem> Workspace<F> {
    /// Create a new Workspace with a custom filesystem implementation
    pub async fn new_with_fs(root: PathBuf, fs: Arc<F>) -> Result<Self> {
        let root = expand_tilde(&root);

        // Get canonical project root, creating parent dirs if needed
        let root = if !root.exists() {
            if let Some(parent) = root.parent() {
                fs.create_dir_all(parent).await?;
            }
            // For non-existent paths, still try to canonicalize the parent and join the last component
            if let Some(parent) = root.parent() {
                let canonical_parent = fs
                    .canonicalize(parent)
                    .await
                    .map_err(|e| eyre!("Failed to canonicalize parent directory: {}", e))?;
                if let Some(file_name) = root.file_name() {
                    canonical_parent.join(file_name)
                } else {
                    root
                }
            } else {
                root
            }
        } else {
            fs.canonicalize(&root)
                .await
                .map_err(|e| eyre!("Failed to canonicalize project root: {}", e))?
        };

        // Get project name from last component
        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                // Fallback for cases where file_name() returns None (like root directories)
                "otto_project".to_string()
            });

        let mut hasher = Sha256::new();
        hasher.update(root.to_string_lossy().as_bytes());
        let hash = hex::encode(hasher.finalize());
        let hash = hash[..8].to_string();

        Self::new_with_hash_and_fs(root, name, hash, fs).await
    }

    pub async fn new_with_hash_and_fs(root: PathBuf, name: String, hash: String, fs: Arc<F>) -> Result<Self> {
        let time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

        let home = resolve_otto_home()?;

        // Build computed paths - one helper decides what a run root is called,
        // so cleanup can find the directory this run creates.
        let project = run_root(&home, &name, &hash);
        let cache = project.join(".cache");

        // Reserve the run directory here rather than letting `init` create it,
        // because reserving a name and creating it have to be one step. Every run
        // starting in the same second used to get the same directory: they raced
        // each other creating `tasks/` (`File exists (os error 17)`), overwrote
        // each other's task logs, and once the runs table stopped rejecting
        // duplicate timestamps, cleaning any one of them deleted the directory the
        // others still pointed at.
        fs.create_dir_all(&project)
            .await
            .map_err(|e| eyre!("Failed to create project directory {}: {}", project.display(), e))?;
        let mut run = project.join(run_dir_name(time, 0));
        for seq in 0..RUN_DIR_ATTEMPTS {
            let candidate = project.join(run_dir_name(time, seq));
            if fs.create_dir_exclusive(&candidate).await? {
                run = candidate;
                break;
            }
            if seq + 1 == RUN_DIR_ATTEMPTS {
                return Err(eyre!(
                    "Failed to reserve a run directory under {} after {RUN_DIR_ATTEMPTS} attempts",
                    project.display()
                ));
            }
        }

        // Try to create default StateManager for production use
        let state_store: Option<Arc<dyn StateStore>> =
            StateManager::try_new().map(|m| Arc::new(m) as Arc<dyn StateStore>);
        if state_store.is_none() {
            log::warn!("This run will not appear in otto History or otto Stats");
        }

        Ok(Self {
            home,
            root,
            hash,
            time,
            project,
            cache,
            run,
            db_run_id: std::sync::Mutex::new(None),
            state_store,
            fs,
        })
    }

    /// Set a custom state store (for testing with MemoryStateStore)
    pub fn with_state_store(mut self, store: Arc<dyn StateStore>) -> Self {
        self.state_store = Some(store);
        self
    }

    /// Set state store to None (disable DB recording)
    pub fn without_state_store(mut self) -> Self {
        self.state_store = None;
        self
    }

    /// Get a reference to the state store (for task recording in scheduler)
    pub fn state_store(&self) -> Option<&Arc<dyn StateStore>> {
        self.state_store.as_ref()
    }

    /// Initialize workspace directories
    pub async fn init(&self) -> Result<()> {
        log::debug!(
            "init: home={} project={} cache={} run={}",
            self.home.display(),
            self.project.display(),
            self.cache.display(),
            self.run.display()
        );
        for path in [&self.home, &self.project, &self.cache, &self.run] {
            self.fs
                .create_dir_all(path)
                .await
                .map_err(|e| eyre!("Failed to create directory {}: {}", path.display(), e))?;
        }

        self.fs
            .create_dir_all(&self.run.join("tasks"))
            .await
            .map_err(|e| eyre!("Failed to create tasks directory: {}", e))?;

        Ok(())
    }

    /// Get a reference to the filesystem
    pub fn fs(&self) -> &Arc<F> {
        &self.fs
    }

    /// Get task directory for current run
    pub fn task(&self, name: &str) -> PathBuf {
        self.run.join("tasks").join(name)
    }

    /// Get path for task script symlink
    pub fn script(&self, task: &str, is_python: bool) -> PathBuf {
        let ext = if is_python { "py" } else { "sh" };
        self.task(task).join(format!("script.{ext}"))
    }

    /// Get path for task output file
    pub fn output(&self, task: &str) -> PathBuf {
        self.task(task).join("output.json")
    }

    /// Get path for task stdout log
    pub fn stdout(&self, task: &str) -> PathBuf {
        self.task(task).join("stdout.log")
    }

    /// Get path for task stderr log
    pub fn stderr(&self, task: &str) -> PathBuf {
        self.task(task).join("stderr.log")
    }

    /// Get path for task artifacts directory
    pub fn artifacts(&self, task: &str) -> PathBuf {
        self.task(task).join("artifacts")
    }

    /// Get path for run metadata files
    pub fn metadata(&self, name: &str) -> PathBuf {
        self.run.join(format!("{name}.yaml"))
    }

    /// Get the project root directory
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn run(&self) -> &PathBuf {
        &self.run
    }

    /// Get the unique hash for this project
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// Get the timestamp for this run
    pub fn timestamp(&self) -> u64 {
        self.time
    }

    /// Get the relative path from project root to a file
    pub fn relative_to_root<P: AsRef<Path>>(&self, path: P) -> Result<PathBuf> {
        path.as_ref()
            .strip_prefix(&self.root)
            .map(|p| p.to_path_buf())
            .map_err(|e| {
                eyre!(
                    "Path {} is not relative to root {}: {}",
                    path.as_ref().display(),
                    self.root.display(),
                    e
                )
            })
    }

    pub fn is_in_project<P: AsRef<Path>>(&self, path: P) -> bool {
        path.as_ref().starts_with(&self.root)
    }

    /// Get a path relative to the project root
    pub fn join_root<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        self.root.join(path)
    }

    pub async fn save_execution_context(&self, context: ExecutionContext) -> Result<()> {
        let run_yaml_path = self.metadata("run");
        let yaml_content =
            yaml_serde::to_string(&context).map_err(|e| eyre!("Failed to serialize execution context: {}", e))?;

        self.fs
            .write(&run_yaml_path, yaml_content.as_bytes())
            .await
            .map_err(|e| eyre!("Failed to write run.yaml: {}", e))?;

        // Also try to record in database (graceful degradation if DB unavailable)
        self.record_run_start_in_db(&context).await;

        Ok(())
    }

    async fn record_run_start_in_db(&self, context: &ExecutionContext) {
        let Some(store) = self.state_store.as_ref() else {
            return;
        };

        // Convert ExecutionContext to RunMetadata. The run directory is
        // recorded, not left to be reconstructed later from a guess.
        //
        // The hostname comes from `current_system_info`, which existed for this
        // and had no production caller: every history row was written with a
        // NULL hostname while `docs/history.md`'s JSON example promised one.
        // The user still comes from the context, which resolved it once at run
        // start and is what `run.yaml` records.
        let (_, hostname) = RunMetadata::current_system_info();
        let metadata = RunMetadata::full(
            context.ottofile.clone(),
            context.hash.clone(),
            context.timestamp,
            Some(context.cwd.clone()),
            Some(context.user.clone()),
            hostname,
            Some(context.args.clone()),
        )
        .with_run_dir(self.run.clone());

        // Try to record - log error but don't fail
        match record_blocking(store, move |store| store.record_run_start(&metadata)).await {
            Ok(run_id) => {
                // Store the run_id for task tracking
                if let Ok(mut db_run_id) = self.db_run_id.lock() {
                    *db_run_id = Some(run_id);
                }
            }
            Err(e) => {
                log::warn!("Failed to record run start in database: {}", e);
            }
        }
    }

    /// Get the database run ID if available
    pub fn db_run_id(&self) -> Option<i64> {
        self.db_run_id.lock().ok().and_then(|guard| *guard)
    }

    /// Mark this run finished in the database.
    ///
    /// Called from run teardown on both the plain and TUI paths. Before that it
    /// had no production caller at all, so every run in the database stayed
    /// `running` forever and `History`, `Stats`, and `Clean` all read from a
    /// table where nothing had ever finished.
    pub async fn record_run_complete_in_db(&self, success: bool) {
        let Some(store) = self.state_store.as_ref() else {
            return;
        };
        let Some(run_id) = self.db_run_id() else {
            // No id means the run start was never recorded; there is nothing to
            // complete, and saying so beats a silent return.
            log::warn!("Run has no database id, so its completion was not recorded");
            return;
        };

        let status = if success {
            super::state::RunStatus::Success
        } else {
            super::state::RunStatus::Failed
        };

        // The size the run directory owns, from the one function that computes
        // it. This used to follow symlinks while `Clean` skipped them, so the
        // two reported different sizes for the same directory.
        let size_bytes = directory_size(&self.run).ok();

        // Try to record - log error but don't fail
        if let Err(e) = record_blocking(store, move |store| {
            store.record_run_complete(run_id, status, size_bytes)
        })
        .await
        {
            log::warn!("Failed to record run completion in database: {}", e);
        }
    }

    pub async fn save_task_context(&self, task_name: &str, context: &ExecutionContext) -> Result<()> {
        let task_run_yaml = self.task(task_name).join("run.yaml");
        let yaml_content =
            yaml_serde::to_string(context).map_err(|e| eyre!("Failed to serialize task context: {}", e))?;

        self.fs
            .write(&task_run_yaml, yaml_content.as_bytes())
            .await
            .map_err(|e| eyre!("Failed to write task run.yaml: {}", e))?;

        Ok(())
    }

    // === NEW METHODS FOR ACTION PROCESSING ===

    /// Get task directory path (alias for existing task() method)
    pub fn task_dir(&self, task_name: &str) -> PathBuf {
        self.task(task_name)
    }

    /// Get task input directory path
    pub fn task_input_dir(&self, task_name: &str) -> PathBuf {
        self.task(task_name).join("inputs")
    }

    /// Get task output directory path
    pub fn task_output_dir(&self, task_name: &str) -> PathBuf {
        self.task(task_name).join("outputs")
    }

    /// Get task output file path
    pub fn task_output_file(&self, task_name: &str) -> PathBuf {
        self.task(task_name).join(format!("output.{task_name}.json"))
    }

    /// Get task input file path for a specific dependency
    pub fn task_input_file(&self, task_name: &str, dep_name: &str) -> PathBuf {
        self.task(task_name).join(format!("input.{dep_name}.json"))
    }

    /// Get task script file path with extension
    pub fn task_script_file(&self, task_name: &str, extension: &str) -> PathBuf {
        self.task(task_name).join(format!("script.{extension}"))
    }

    /// Get relative path from task script to cache file
    /// Returns: `../../../.cache/<filename>`
    pub fn relative_script_cache_path(&self, cache_file: &Path) -> PathBuf {
        // Script is at: <run>/tasks/<task>/script.{sh,py}
        // Cache is at: <project>/.cache/<hash>.{sh,py}
        // Relative: ../../../.cache/<filename>
        let mut relative_path = PathBuf::from("../../..");
        relative_path.push(".cache");
        if let Some(filename) = cache_file.file_name() {
            relative_path.push(filename);
        }
        relative_path
    }

    /// Get relative path from task input to dependency output
    /// Returns: `../<dep_name>/output.<dep_name>.json`
    pub fn relative_task_dependency_path(&self, dep_name: &str) -> PathBuf {
        PathBuf::from("..")
            .join(dep_name)
            .join(format!("output.{dep_name}.json"))
    }

    /// Get task output .env file path (for jq-free bash serialization)
    pub fn task_output_env_file(&self, task_name: &str) -> PathBuf {
        self.task(task_name).join(format!("output.{task_name}.env"))
    }

    /// Get task input .env file path for a specific dependency (for jq-free bash deserialization)
    pub fn task_input_env_file(&self, task_name: &str, dep_name: &str) -> PathBuf {
        self.task(task_name).join(format!("input.{dep_name}.env"))
    }

    /// Get the current run directory
    pub fn current_run_dir(&self) -> &PathBuf {
        &self.run
    }

    /// Get the project root directory (alias for root())
    pub fn project_root(&self) -> &PathBuf {
        &self.root
    }

    /// Get path for bash builtin functions
    pub fn bash_builtins(&self) -> PathBuf {
        self.project.join("builtins.sh")
    }

    /// Get path for python builtin functions
    pub fn python_builtins(&self) -> PathBuf {
        self.project.join("builtins.py")
    }

    /// Get the cache directory for this workspace
    pub fn cache_dir(&self) -> &PathBuf {
        &self.cache
    }
}

#[path = "workspace_tests.rs"]
mod tests;
