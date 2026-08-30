use crate::executor::layout::{resolve_otto_home, run_root};
use crate::ports::{FileSystem, RealFs, StateStore, record_blocking};
use expanduser::expanduser;
use eyre::{Result, eyre};
use serde::{Deserialize, Serialize};
use serde_yaml;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use super::state::{RunMetadata, StateManager};

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

impl ExecutionContext {
    pub fn new() -> Self {
        Self {
            prog: "otto".to_string(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            user: std::env::var("USER").unwrap_or_else(|_| "unknown".to_string()),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
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
        let root = expanduser(root.to_string_lossy())?;

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
        let hash = format!("{:x}", hasher.finalize());
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
        let run = project.join(time.to_string());

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

    /// Get path for a cached script
    pub fn script_cache(&self, task: &str, hash: &str) -> PathBuf {
        self.cache.join(task).join(hash)
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

    pub async fn verify_task(&self, name: &str) -> Result<()> {
        let task_dir = self.task(name);

        if !self.fs.exists(&task_dir).await {
            return Err(eyre!("Task directory does not exist: {}", task_dir.display()));
        }

        let script = task_dir.join("script.*");
        if !self.fs.exists(&script).await {
            return Err(eyre!("Task script not found: {}", script.display()));
        }

        let script_target = self
            .fs
            .read_link(&script)
            .await
            .map_err(|e| eyre!("Failed to read script symlink: {}", e))?;

        if !self.fs.exists(&script_target).await {
            return Err(eyre!("Script target does not exist: {}", script_target.display()));
        }

        Ok(())
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
            serde_yaml::to_string(&context).map_err(|e| eyre!("Failed to serialize execution context: {}", e))?;

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
        let metadata = RunMetadata::full(
            context.ottofile.clone(),
            context.hash.clone(),
            context.timestamp,
            Some(context.cwd.clone()),
            Some(context.user.clone()),
            None, // hostname not in ExecutionContext yet
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

        // Try to calculate directory size
        let size_bytes = Self::calculate_directory_size(&self.run).ok();

        // Try to record - log error but don't fail
        if let Err(e) = record_blocking(store, move |store| {
            store.record_run_complete(run_id, status, size_bytes)
        })
        .await
        {
            log::warn!("Failed to record run completion in database: {}", e);
        }
    }

    fn calculate_directory_size(path: &Path) -> Result<u64> {
        let mut total = 0u64;
        if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    total += Self::calculate_directory_size(&entry_path)?;
                } else {
                    total += entry.metadata()?.len();
                }
            }
        }
        Ok(total)
    }

    pub async fn save_task_context(&self, task_name: &str, context: &ExecutionContext) -> Result<()> {
        let task_run_yaml = self.task(task_name).join("run.yaml");
        let yaml_content =
            serde_yaml::to_string(context).map_err(|e| eyre!("Failed to serialize task context: {}", e))?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::MemFs;
    use serial_test::serial;
    use tempfile::TempDir;

    // === MemFs-based tests (fast, no real I/O) ===

    #[tokio::test]
    #[serial]
    async fn test_workspace_init_with_memfs() -> Result<()> {
        let fs = Arc::new(MemFs::new());
        // Pre-create the root directory
        fs.create_dir_all(Path::new("/project")).await?;

        unsafe {
            std::env::set_var("OTTO_HOME", "/otto-home");
        }

        let ws = Workspace::new_with_hash_and_fs(
            PathBuf::from("/project"),
            "myproject".to_string(),
            "abc12345".to_string(),
            fs.clone(),
        )
        .await?;

        ws.init().await?;

        // Verify directories were created in MemFs
        assert!(fs.is_dir(Path::new("/otto-home")).await);
        assert!(fs.is_dir(&ws.project).await);
        assert!(fs.is_dir(&ws.cache).await);
        assert!(fs.is_dir(&ws.run).await);
        assert!(fs.is_dir(&ws.run.join("tasks")).await);

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_save_execution_context_with_memfs() -> Result<()> {
        let fs = Arc::new(MemFs::new());
        fs.create_dir_all(Path::new("/project")).await?;

        unsafe {
            std::env::set_var("OTTO_HOME", "/otto-home");
        }

        let ws = Workspace::new_with_hash_and_fs(
            PathBuf::from("/project"),
            "myproject".to_string(),
            "abc12345".to_string(),
            fs.clone(),
        )
        .await?;
        ws.init().await?;

        let context = ExecutionContext {
            prog: "otto".to_string(),
            cwd: PathBuf::from("/project"),
            user: "testuser".to_string(),
            timestamp: 1234567890,
            hash: "abc12345".to_string(),
            ottofile: Some(PathBuf::from("/project/.otto.yml")),
            args: vec!["otto".to_string(), "build".to_string()],
        };

        ws.save_execution_context(context).await?;

        // Verify run.yaml was written
        let run_yaml_path = ws.metadata("run");
        assert!(fs.exists(&run_yaml_path).await);

        let content = fs.read_to_string(&run_yaml_path).await?;
        assert!(content.contains("prog: otto"));
        assert!(content.contains("user: testuser"));

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_save_task_context_with_memfs() -> Result<()> {
        let fs = Arc::new(MemFs::new());
        fs.create_dir_all(Path::new("/project")).await?;

        unsafe {
            std::env::set_var("OTTO_HOME", "/otto-home");
        }

        let ws = Workspace::new_with_hash_and_fs(
            PathBuf::from("/project"),
            "myproject".to_string(),
            "abc12345".to_string(),
            fs.clone(),
        )
        .await?;
        ws.init().await?;

        // Create task directory
        let task_dir = ws.task("build");
        fs.create_dir_all(&task_dir).await?;

        let context = ExecutionContext::new();
        ws.save_task_context("build", &context).await?;

        // Verify task run.yaml was written
        let task_run_yaml = task_dir.join("run.yaml");
        assert!(fs.exists(&task_run_yaml).await);

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_workspace_path_helpers() -> Result<()> {
        let fs = Arc::new(MemFs::new());
        fs.create_dir_all(Path::new("/project")).await?;

        unsafe {
            std::env::set_var("OTTO_HOME", "/otto-home");
        }

        let ws = Workspace::new_with_hash_and_fs(
            PathBuf::from("/project"),
            "myproject".to_string(),
            "abc12345".to_string(),
            fs.clone(),
        )
        .await?;

        // Test all path helper methods
        assert_eq!(ws.root(), &PathBuf::from("/project"));
        assert_eq!(ws.hash(), "abc12345");
        assert!(ws.script("task1", true).to_string_lossy().ends_with("script.py"));
        assert!(ws.script("task1", false).to_string_lossy().ends_with("script.sh"));
        assert!(ws.output("task1").to_string_lossy().ends_with("output.json"));
        assert!(ws.stdout("task1").to_string_lossy().ends_with("stdout.log"));
        assert!(ws.stderr("task1").to_string_lossy().ends_with("stderr.log"));
        assert!(ws.artifacts("task1").to_string_lossy().ends_with("artifacts"));

        // Test action processing path helpers
        assert!(ws.task_dir("task1").to_string_lossy().contains("tasks/task1"));
        assert!(ws.task_input_dir("task1").to_string_lossy().ends_with("inputs"));
        assert!(ws.task_output_dir("task1").to_string_lossy().ends_with("outputs"));
        assert!(
            ws.task_output_file("task1")
                .to_string_lossy()
                .contains("output.task1.json")
        );
        assert!(
            ws.task_input_file("task1", "dep1")
                .to_string_lossy()
                .contains("input.dep1.json")
        );
        assert!(
            ws.task_script_file("task1", "sh")
                .to_string_lossy()
                .ends_with("script.sh")
        );
        assert!(ws.bash_builtins().to_string_lossy().ends_with("builtins.sh"));
        assert!(ws.python_builtins().to_string_lossy().ends_with("builtins.py"));

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_relative_paths() -> Result<()> {
        let fs = Arc::new(MemFs::new());
        fs.create_dir_all(Path::new("/project/src")).await?;

        unsafe {
            std::env::set_var("OTTO_HOME", "/otto-home");
        }

        let ws = Workspace::new_with_hash_and_fs(
            PathBuf::from("/project"),
            "myproject".to_string(),
            "abc12345".to_string(),
            fs.clone(),
        )
        .await?;

        // Test relative_to_root
        let rel = ws.relative_to_root("/project/src/main.rs")?;
        assert_eq!(rel, PathBuf::from("src/main.rs"));

        // Test is_in_project
        assert!(ws.is_in_project("/project/src/main.rs"));
        assert!(!ws.is_in_project("/other/file.rs"));

        // Test join_root
        assert_eq!(ws.join_root("src/lib.rs"), PathBuf::from("/project/src/lib.rs"));

        // Test relative_script_cache_path
        let cache_path = ws.script_cache("task1", "hash123");
        let relative = ws.relative_script_cache_path(&cache_path);
        assert!(relative.to_string_lossy().contains(".cache"));

        // Test relative_task_dependency_path
        let dep_path = ws.relative_task_dependency_path("dep1");
        assert!(dep_path.to_string_lossy().contains("dep1"));
        assert!(dep_path.to_string_lossy().contains("output.dep1.json"));

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_verify_task_with_memfs() -> Result<()> {
        let fs = Arc::new(MemFs::new());
        fs.create_dir_all(Path::new("/project")).await?;

        unsafe {
            std::env::set_var("OTTO_HOME", "/otto-home");
        }

        let ws = Workspace::new_with_hash_and_fs(
            PathBuf::from("/project"),
            "myproject".to_string(),
            "abc12345".to_string(),
            fs.clone(),
        )
        .await?;
        ws.init().await?;

        // verify_task should fail when task dir doesn't exist
        let result = ws.verify_task("nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));

        Ok(())
    }

    // === Real filesystem tests (integration tests) ===

    #[tokio::test]
    #[serial]
    async fn test_workspace_creation() -> Result<()> {
        let temp = TempDir::new()?;
        let root = temp.path().to_path_buf();

        // Set up isolated test workspace
        let otto_home = root.join(".otto");
        unsafe {
            std::env::set_var("OTTO_HOME", &otto_home);
        }

        let ws = Workspace::new(root.clone()).await?;
        ws.init().await?;

        assert!(ws.home.exists());
        assert!(ws.project.exists());
        assert!(ws.cache.exists());
        assert!(ws.run.exists());
        assert!(ws.run.join("tasks").exists());

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_task_paths() -> Result<()> {
        let temp = TempDir::new()?;
        let root = temp.path().to_path_buf();

        // Set up isolated test workspace
        let otto_home = root.join(".otto");
        unsafe {
            std::env::set_var("OTTO_HOME", &otto_home);
        }

        let ws = Workspace::new(root.clone()).await?;
        ws.init().await?;

        let task = "test_task";
        let script_hash = "abcd1234";

        assert!(ws.script_cache(task, script_hash).starts_with(&ws.cache));
        assert!(ws.task(task).starts_with(&ws.run));
        assert!(ws.script(task, false).ends_with("script.sh"));
        assert!(ws.script(task, true).ends_with("script.py"));
        assert!(ws.output(task).ends_with("output.json"));
        assert!(ws.stdout(task).ends_with("stdout.log"));
        assert!(ws.stderr(task).ends_with("stderr.log"));
        assert!(ws.artifacts(task).ends_with("artifacts"));

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_metadata_paths() -> Result<()> {
        let temp = TempDir::new()?;
        let root = temp.path().to_path_buf();

        // Set up isolated test workspace
        let otto_home = root.join(".otto");
        unsafe {
            std::env::set_var("OTTO_HOME", &otto_home);
        }

        let ws = Workspace::new(root.clone()).await?;

        assert!(ws.metadata("run").ends_with("run.yaml"));
        assert!(ws.metadata("env").ends_with("env.yaml"));
        assert!(ws.metadata("cmdline").ends_with("cmdline.yaml"));

        Ok(())
    }

    // === Additional MemFs tests for remaining methods ===

    #[tokio::test]
    #[serial]
    async fn test_env_file_paths_with_memfs() -> Result<()> {
        let fs = Arc::new(MemFs::new());
        fs.create_dir_all(Path::new("/project")).await?;

        unsafe {
            std::env::set_var("OTTO_HOME", "/otto-home");
        }

        let ws = Workspace::new_with_hash_and_fs(
            PathBuf::from("/project"),
            "myproject".to_string(),
            "abc12345".to_string(),
            fs.clone(),
        )
        .await?;

        // Test task_output_env_file
        let output_env = ws.task_output_env_file("build");
        assert!(output_env.to_string_lossy().contains("output.build.env"));

        // Test task_input_env_file
        let input_env = ws.task_input_env_file("build", "compile");
        assert!(input_env.to_string_lossy().contains("input.compile.env"));

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_workspace_accessors_with_memfs() -> Result<()> {
        let fs = Arc::new(MemFs::new());
        fs.create_dir_all(Path::new("/project")).await?;

        unsafe {
            std::env::set_var("OTTO_HOME", "/otto-home");
        }

        let ws = Workspace::new_with_hash_and_fs(
            PathBuf::from("/project"),
            "myproject".to_string(),
            "abc12345".to_string(),
            fs.clone(),
        )
        .await?;

        // Test current_run_dir (alias for run)
        assert_eq!(ws.current_run_dir(), ws.run());

        // Test project_root (alias for root)
        assert_eq!(ws.project_root(), ws.root());

        // Test cache_dir
        assert!(ws.cache_dir().to_string_lossy().contains(".cache"));

        // Test timestamp
        assert!(ws.timestamp() > 0);

        // Test fs() accessor
        let _fs_ref = ws.fs();

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_db_run_id_initially_none() -> Result<()> {
        let fs = Arc::new(MemFs::new());
        fs.create_dir_all(Path::new("/project")).await?;

        unsafe {
            std::env::set_var("OTTO_HOME", "/otto-home");
        }

        let ws = Workspace::new_with_hash_and_fs(
            PathBuf::from("/project"),
            "myproject".to_string(),
            "abc12345".to_string(),
            fs.clone(),
        )
        .await?;

        // Before any DB interaction, run ID should be None
        assert!(ws.db_run_id().is_none());

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_execution_context_default() {
        let ctx = ExecutionContext::default();
        assert_eq!(ctx.prog, "otto");
        assert!(!ctx.user.is_empty());
        assert!(ctx.timestamp > 0);
    }

    #[tokio::test]
    #[serial]
    async fn test_execution_context_new() {
        let ctx = ExecutionContext::new();
        assert_eq!(ctx.prog, "otto");
        assert_eq!(ctx.hash, "test");
        assert!(ctx.args.contains(&"otto".to_string()));
    }

    #[tokio::test]
    #[serial]
    async fn test_verify_task_missing_script_with_memfs() -> Result<()> {
        let fs = Arc::new(MemFs::new());
        fs.create_dir_all(Path::new("/project")).await?;

        unsafe {
            std::env::set_var("OTTO_HOME", "/otto-home");
        }

        let ws = Workspace::new_with_hash_and_fs(
            PathBuf::from("/project"),
            "myproject".to_string(),
            "abc12345".to_string(),
            fs.clone(),
        )
        .await?;
        ws.init().await?;

        // Create the task directory but not the script
        let task_dir = ws.task("mytask");
        fs.create_dir_all(&task_dir).await?;

        // Verify should fail because script is missing
        let result = ws.verify_task("mytask").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("script"));

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_relative_to_root_outside_project() -> Result<()> {
        let fs = Arc::new(MemFs::new());
        fs.create_dir_all(Path::new("/project")).await?;

        unsafe {
            std::env::set_var("OTTO_HOME", "/otto-home");
        }

        let ws = Workspace::new_with_hash_and_fs(
            PathBuf::from("/project"),
            "myproject".to_string(),
            "abc12345".to_string(),
            fs.clone(),
        )
        .await?;

        // Path outside project should error
        let rel = ws.relative_to_root("/other/path/file.txt");
        assert!(rel.is_err());
        assert!(rel.unwrap_err().to_string().contains("not relative to root"));

        Ok(())
    }

    // === StateStore Integration Tests ===

    #[tokio::test]
    #[serial]
    async fn test_workspace_with_memory_state_store() -> Result<()> {
        use crate::ports::MemoryStateStore;

        let fs = Arc::new(MemFs::new());
        fs.create_dir_all(Path::new("/project")).await?;

        unsafe {
            std::env::set_var("OTTO_HOME", "/otto-home");
        }

        let store = Arc::new(MemoryStateStore::new());

        let ws = Workspace::new_with_hash_and_fs(
            PathBuf::from("/project"),
            "myproject".to_string(),
            "abc12345".to_string(),
            fs.clone(),
        )
        .await?
        .with_state_store(store.clone());

        ws.init().await?;

        // Save execution context - this should record to MemoryStateStore
        let context = ExecutionContext {
            prog: "otto".to_string(),
            cwd: PathBuf::from("/project"),
            user: "testuser".to_string(),
            timestamp: 1234567890,
            hash: "abc12345".to_string(),
            ottofile: Some(PathBuf::from("/project/.otto.yml")),
            args: vec!["otto".to_string(), "build".to_string()],
        };

        ws.save_execution_context(context).await?;

        // Verify run was recorded in MemoryStateStore
        assert!(ws.db_run_id().is_some());

        // Verify we can query the store
        let runs = store.get_recent_runs(10, None)?;
        assert_eq!(runs.len(), 1);

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_workspace_without_state_store() -> Result<()> {
        let fs = Arc::new(MemFs::new());
        fs.create_dir_all(Path::new("/project")).await?;

        unsafe {
            std::env::set_var("OTTO_HOME", "/otto-home");
        }

        let ws = Workspace::new_with_hash_and_fs(
            PathBuf::from("/project"),
            "myproject".to_string(),
            "abc12345".to_string(),
            fs.clone(),
        )
        .await?
        .without_state_store();

        ws.init().await?;

        // Save execution context - should work even without state store
        let context = ExecutionContext::new();
        ws.save_execution_context(context).await?;

        // No run_id since no state store
        assert!(ws.db_run_id().is_none());

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_record_run_complete_with_memory_store() -> Result<()> {
        use crate::ports::MemoryStateStore;

        let fs = Arc::new(MemFs::new());
        fs.create_dir_all(Path::new("/project")).await?;

        unsafe {
            std::env::set_var("OTTO_HOME", "/otto-home");
        }

        let store = Arc::new(MemoryStateStore::new());

        let ws = Workspace::new_with_hash_and_fs(
            PathBuf::from("/project"),
            "myproject".to_string(),
            "abc12345".to_string(),
            fs.clone(),
        )
        .await?
        .with_state_store(store.clone());

        ws.init().await?;

        // First record run start
        let context = ExecutionContext::new();
        ws.save_execution_context(context).await?;

        // Now record completion
        ws.record_run_complete_in_db(true).await;

        // Verify the run was marked complete
        let runs = store.get_recent_runs(10, None)?;
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0].status,
            crate::executor::state::RunStatus::Success,
            "a finished run must not stay `running`"
        );
        assert!(runs[0].ended_at.is_some());
        assert_eq!(
            runs[0].run_dir,
            Some(PathBuf::from("/otto-home/myproject-abc12345").join(ws.time.to_string())),
            "the run records the directory it wrote into"
        );

        Ok(())
    }
}
