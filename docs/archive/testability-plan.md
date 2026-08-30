# Otto Testability Plan: Complete DI Wiring

## Goal

Achieve 90%+ test coverage by completing the dependency injection architecture per rust-cli-coder principles.

## Current State

- Coverage: 73.57%
- FileSystem trait exists but ActionProcessor bypasses it
- No abstractions for HTTP, Database, or Process execution

---

## Phase 1: Wire FileSystem Through ActionProcessor

**Problem:** ActionProcessor has `F: FileSystem` generic but calls `std::fs` directly.

**File:** `src/executor/action.rs`

### 1.1 Make FileSystem Sync (or ActionProcessor Async)

The current FileSystem trait is async, but ActionProcessor is sync. Options:

**Option A: Add sync methods to FileSystem trait**
```rust
pub trait FileSystem: Send + Sync {
    // Async methods (existing)
    async fn read_to_string(&self, path: &Path) -> Result<String>;
    async fn write(&self, path: &Path, contents: &[u8]) -> Result<()>;

    // Sync methods (new)
    fn read_to_string_sync(&self, path: &Path) -> Result<String>;
    fn write_sync(&self, path: &Path, contents: &[u8]) -> Result<()>;
    fn create_dir_all_sync(&self, path: &Path) -> Result<()>;
    fn exists_sync(&self, path: &Path) -> bool;
    fn metadata_sync(&self, path: &Path) -> Result<FileMetadata>;
    fn set_permissions_sync(&self, path: &Path, mode: u32) -> Result<()>;
    fn remove_file_sync(&self, path: &Path) -> Result<()>;
    fn symlink_sync(&self, original: &Path, link: &Path) -> Result<()>;
}
```

**Option B: Make ActionProcessor async** (more invasive)

**Recommendation:** Option A - add sync methods.

### 1.2 Update ActionProcessor to Use self.fs

Replace all direct `std::fs` calls:

```rust
// BEFORE (current code)
impl<F: FileSystem> ActionProcessor<F> {
    fn write_script<T: ScriptProcessor>(&self, processor: &T, script: &str) -> Result<PathBuf> {
        std::fs::create_dir_all(self.workspace.cache_dir())?;  // Direct call!
        std::fs::write(&cache_file, script)?;                   // Direct call!
        // ...
    }
}

// AFTER (using injected fs)
impl<F: FileSystem> ActionProcessor<F> {
    fn write_script<T: ScriptProcessor>(&self, processor: &T, script: &str) -> Result<PathBuf> {
        self.workspace.fs().create_dir_all_sync(self.workspace.cache_dir())?;
        self.workspace.fs().write_sync(&cache_file, script.as_bytes())?;
        // ...
    }
}
```

### 1.3 Specific Replacements in action.rs

| Line | Current | Replace With |
|------|---------|--------------|
| 101 | `std::fs::create_dir_all(...)` | `self.workspace.fs().create_dir_all_sync(...)` |
| 105 | `std::fs::write(...)` | `self.workspace.fs().write_sync(...)` |
| 111-113 | `std::fs::metadata(...).permissions()` | `self.workspace.fs().metadata_sync(...)` + `set_permissions_sync` |
| 123 | `std::fs::create_dir_all(parent)` | `self.workspace.fs().create_dir_all_sync(parent)` |
| 126-127 | `path.exists()` + `std::fs::remove_file` | `self.workspace.fs().exists_sync()` + `remove_file_sync()` |
| 135 | `fs::symlink(...)` | `self.workspace.fs().symlink_sync(...)` |
| 140 | `std::fs::copy(...)` | `self.workspace.fs().copy_sync(...)` |
| 316 | `std::fs::create_dir_all(parent)` | `self.workspace.fs().create_dir_all_sync(parent)` |
| 450 | `std::fs::write(&builtins_path, ...)` | `self.workspace.fs().write_sync(...)` |
| 456-458 | `std::fs::metadata` + `set_permissions` | `self.workspace.fs().set_permissions_sync(...)` |
| 625 | `std::fs::create_dir_all(parent)` | `self.workspace.fs().create_dir_all_sync(parent)` |
| 704 | `std::fs::write(&builtins_path, ...)` | `self.workspace.fs().write_sync(...)` |

### 1.4 Add Workspace.fs() Accessor

```rust
impl<F: FileSystem> Workspace<F> {
    pub fn fs(&self) -> &F {
        &*self.fs
    }
}
```

### 1.5 Add Tests

```rust
#[test]
fn test_action_processor_with_memfs() {
    let fs = Arc::new(MemFs::new());
    let workspace = Workspace::new_with_fs(PathBuf::from("/project"), fs.clone());
    let processor = ActionProcessor::new(workspace, "test_task").unwrap();

    let task = Task::new(...);
    let result = processor.process("echo hello", &task).unwrap();

    // Verify script was written to MemFs
    assert!(fs.exists_sync(&result.path()));
}
```

---

## Phase 2: Add DatabasePort Trait

**Problem:** `state/manager.rs` talks directly to SQLite.

**File:** `src/executor/state/manager.rs`

### 2.1 Create Database Port

**File:** `src/ports/db.rs`

```rust
use eyre::Result;

pub trait StateStore: Send + Sync {
    fn record_run_start(&self, metadata: &RunMetadata) -> Result<i64>;
    fn record_run_complete(&self, timestamp: u64, status: RunStatus, size: Option<u64>) -> Result<()>;
    fn record_task_start(&self, run_id: i64, task_name: &str, script_hash: Option<&str>) -> Result<i64>;
    fn record_task_complete(&self, task_id: i64, exit_code: i32, status: TaskStatus) -> Result<()>;
    fn get_recent_runs(&self, limit: usize, project: Option<&str>) -> Result<Vec<RunRecord>>;
    fn get_run_tasks(&self, run_id: i64) -> Result<Vec<TaskRecord>>;
    fn get_overall_stats(&self) -> Result<OverallStats>;
    fn delete_run(&self, timestamp: u64, delete_fs: bool) -> Result<Option<RunRecord>>;
}

// Real implementation wraps StateManager
pub struct SqliteStateStore {
    manager: StateManager,
}

// Test fake
#[cfg(test)]
pub struct MemoryStateStore {
    runs: RefCell<Vec<RunRecord>>,
    tasks: RefCell<Vec<TaskRecord>>,
}
```

### 2.2 Refactor StateManager

Current StateManager becomes the implementation of StateStore trait.

### 2.3 Update Consumers

Workspace and scheduler accept `S: StateStore` instead of hardcoding StateManager.

---

## Phase 3: Add HttpPort Trait

**Problem:** `cli/commands/upgrade.rs` does HTTP with no abstraction.

**File:** `src/cli/commands/upgrade.rs`

### 3.1 Create HTTP Port

**File:** `src/ports/http.rs`

```rust
use eyre::Result;

pub trait ReleaseFetcher: Send + Sync {
    fn get_latest_release(&self, repo: &str) -> Result<ReleaseInfo>;
    fn download_asset(&self, url: &str, dest: &Path) -> Result<u64>;
}

pub struct ReleaseInfo {
    pub tag: String,
    pub assets: Vec<AssetInfo>,
}

pub struct AssetInfo {
    pub name: String,
    pub download_url: String,
    pub size: u64,
}

// Real implementation using reqwest
pub struct GithubReleaseFetcher {
    client: reqwest::blocking::Client,
}

// Test fake
#[cfg(test)]
pub struct MockReleaseFetcher {
    pub latest_release: Option<ReleaseInfo>,
    pub should_fail: bool,
}
```

### 3.2 Refactor upgrade.rs

```rust
pub fn execute_upgrade_command<R: ReleaseFetcher>(
    args: &[String],
    fetcher: &R,
) -> Result<()> {
    let release = fetcher.get_latest_release("otto-rs/otto")?;
    // ...
}
```

### 3.3 Add Tests

```rust
#[test]
fn test_upgrade_when_already_latest() {
    let fetcher = MockReleaseFetcher {
        latest_release: Some(ReleaseInfo {
            tag: env!("CARGO_PKG_VERSION").to_string(),
            assets: vec![],
        }),
        should_fail: false,
    };

    let result = check_for_upgrade(&fetcher);
    assert!(result.is_ok());
    assert!(!result.unwrap().needs_upgrade);
}

#[test]
fn test_upgrade_handles_network_error() {
    let fetcher = MockReleaseFetcher {
        latest_release: None,
        should_fail: true,
    };

    let result = check_for_upgrade(&fetcher);
    assert!(result.is_err());
}
```

---

## Phase 4: Add ProcessPort Trait

**Problem:** `scheduler.rs` spawns real processes with `tokio::process::Command`.

**File:** `src/executor/scheduler.rs`

### 4.1 Create Process Port

**File:** `src/ports/process.rs`

```rust
use eyre::Result;
use std::path::Path;

#[async_trait]
pub trait ProcessRunner: Send + Sync {
    async fn run_script(
        &self,
        interpreter: &str,
        script_path: &Path,
        env: &HashMap<String, String>,
        cwd: &Path,
    ) -> Result<ProcessOutput>;
}

pub struct ProcessOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

// Real implementation
pub struct RealProcessRunner;

#[async_trait]
impl ProcessRunner for RealProcessRunner {
    async fn run_script(&self, interpreter: &str, script_path: &Path, env: &HashMap<String, String>, cwd: &Path) -> Result<ProcessOutput> {
        let output = tokio::process::Command::new(interpreter)
            .arg(script_path)
            .envs(env)
            .current_dir(cwd)
            .output()
            .await?;

        Ok(ProcessOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

// Test fake
#[cfg(test)]
pub struct MockProcessRunner {
    pub responses: RefCell<HashMap<PathBuf, ProcessOutput>>,
}

#[cfg(test)]
impl MockProcessRunner {
    pub fn with_success(script: &str) -> Self {
        let mut responses = HashMap::new();
        responses.insert(
            PathBuf::from(script),
            ProcessOutput { exit_code: 0, stdout: vec![], stderr: vec![] },
        );
        Self { responses: RefCell::new(responses) }
    }
}
```

### 4.2 Refactor TaskScheduler

```rust
pub struct TaskScheduler<F: FileSystem = RealFs, P: ProcessRunner = RealProcessRunner> {
    workspace: Arc<Workspace<F>>,
    process_runner: Arc<P>,
    // ...
}

impl<F: FileSystem, P: ProcessRunner> TaskScheduler<F, P> {
    async fn execute_task(&self, task: &Task) -> Result<TaskResult> {
        let output = self.process_runner.run_script(
            &interpreter,
            &script_path,
            &env_vars,
            self.workspace.root(),
        ).await?;

        Ok(TaskResult {
            exit_code: output.exit_code,
            // ...
        })
    }
}
```

### 4.3 Add Tests

```rust
#[tokio::test]
async fn test_scheduler_with_mock_process() {
    let fs = Arc::new(MemFs::new());
    let process_runner = Arc::new(MockProcessRunner::with_success("build.sh"));

    let workspace = Workspace::new_with_fs(PathBuf::from("/project"), fs);
    let scheduler = TaskScheduler::new_with_deps(
        vec![task],
        workspace,
        process_runner,
        ExecutionContext::new(),
        4,
        false,
    ).await?;

    let results = scheduler.run().await?;
    assert!(results.all_succeeded());
}
```

---

## Implementation Order

1. **Phase 1** (ActionProcessor) - Highest impact, enables MemFs testing of script generation
2. **Phase 4** (ProcessRunner) - Enables testing scheduler logic without spawning processes
3. **Phase 2** (StateStore) - Enables testing history/stats commands
4. **Phase 3** (ReleaseFetcher) - Enables testing upgrade command

## Expected Coverage Impact

| Phase | Files Affected | Current | Expected |
|-------|---------------|---------|----------|
| 1 | action.rs | 95% | 98% |
| 2 | state/manager.rs, history.rs, stats.rs | 19-65% | 80%+ |
| 3 | upgrade.rs | 18% | 85%+ |
| 4 | scheduler.rs | 90% | 95%+ |
| **Total** | | 73.57% | **90%+** |

## Files to Create

```
src/ports/
├── mod.rs      # Add: db, http, process
├── fs.rs       # Update: add sync methods
├── db.rs       # New: StateStore trait
├── http.rs     # New: ReleaseFetcher trait
└── process.rs  # New: ProcessRunner trait
```

## Verification

After each phase:

```bash
cargo test
otto cov
```

Target: 90%+ line coverage with no E2E tests for code paths.
