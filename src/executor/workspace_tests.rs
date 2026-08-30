#![cfg(test)]

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
