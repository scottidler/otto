use assert_fs::TempDir;
use eyre::Result;
use serial_test::serial;
use std::fs;
use std::path::PathBuf;

/// Helper to set up a test-specific database path and workspace
fn setup_test_db(temp_dir: &std::path::Path) -> PathBuf {
    let db_path = temp_dir.join("test_otto.db");
    let otto_home = temp_dir.join(".otto");
    // SAFETY: This is safe in tests because we control the execution environment
    // and tests are isolated. The env var is set before any StateManager is created.
    unsafe {
        std::env::set_var("OTTO_DB_PATH", &db_path);
        std::env::set_var("OTTO_HOME", &otto_home);
    }
    db_path
}

#[tokio::test]
#[serial]
async fn test_execution_context_saved_with_ottofile_path() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Set up isolated test database
    setup_test_db(temp_path);

    // Create a simple ottofile
    let ottofile_content = r#"
tasks:
  simple:
    help: "A simple task"
    action: |
      echo "Hello from simple task"
"#;
    let ottofile_path = temp_path.join("otto.yml");
    fs::write(&ottofile_path, ottofile_content)?;

    // Parse the ottofile and execute a task
    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "simple".to_string(),
    ];

    let mut parser = otto::cli::parser::Parser::new(args)?;
    let (tasks, hash, parsed_ottofile_path, jobs, _, _) = parser.parse()?.into_run()?.into_parts();

    // Verify jobs parameter is valid
    assert!(jobs > 0, "Jobs should be a positive number");

    // Filter out built-in commands for normal execution
    let execution_tasks: Vec<_> = tasks
        .into_iter()
        .filter(|task| task.name != "graph" && task.name != "clean")
        .collect();

    if !execution_tasks.is_empty() {
        let cwd = temp_path.to_path_buf();
        let workspace = otto::executor::workspace::Workspace::new(cwd).await?;
        workspace.init().await?;

        // Create execution context with ottofile path
        let mut execution_context = otto::executor::workspace::ExecutionContext::new();
        execution_context.ottofile = parsed_ottofile_path.clone();
        execution_context.hash = hash.clone();

        // Save execution context
        workspace.save_execution_context(execution_context.clone()).await?;

        // Verify run.yaml was created and contains ottofile path
        let run_yaml_path = workspace.run().join("run.yaml");
        assert!(run_yaml_path.exists(), "run.yaml should exist");

        let run_yaml_content = fs::read_to_string(&run_yaml_path)?;
        assert!(
            run_yaml_content.contains("ottofile:"),
            "run.yaml should contain ottofile field"
        );
        assert!(
            run_yaml_content.contains("otto.yml"),
            "run.yaml should contain the ottofile path"
        );

        // Parse and verify the content
        let saved_context: otto::executor::workspace::ExecutionContext = yaml_serde::from_str(&run_yaml_content)?;
        assert_eq!(saved_context.ottofile, parsed_ottofile_path);
        assert_eq!(saved_context.hash, hash);

        // Convert parser tasks to executor tasks (using jobs for scheduler creation)
        let executor_tasks: Vec<otto::executor::Task> = execution_tasks
            .into_iter()
            .map(|parser_task| {
                // Derive parent for subtasks (names with colons like "install:td")
                let parent = if parser_task.name.contains(':') {
                    parser_task.name.split(':').next().map(|s| s.to_string())
                } else {
                    None
                };
                otto::executor::Task::new(
                    parser_task.name,
                    parent,
                    parser_task.task_deps,
                    parser_task.file_deps,
                    parser_task.output_deps,
                    parser_task.envs,
                    parser_task.values,
                    parser_task.action,
                )
            })
            .collect();

        // Create task scheduler with jobs parameter
        use std::sync::Arc;
        let _scheduler =
            otto::executor::TaskScheduler::new(executor_tasks, Arc::new(workspace), execution_context, jobs, false)
                .await?;

        // Note: We don't actually execute tasks in this test since it would require
        // real task execution infrastructure, but we verify the full setup including
        // the jobs parameter being properly threaded through
    }

    temp_dir.close()?;
    Ok(())
}

#[tokio::test]
#[serial]
async fn test_execution_context_hash_matches_ottofile() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Set up isolated test database
    setup_test_db(temp_path);

    // Create a simple ottofile
    let ottofile_content = r#"
tasks:
  verify:
    help: "Verify hash task"
    action: |
      echo "Verifying hash"
"#;
    let ottofile_path = temp_path.join("otto.yml");
    fs::write(&ottofile_path, ottofile_content)?;

    // Parse the ottofile
    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "verify".to_string(),
    ];

    let mut parser = otto::cli::parser::Parser::new(args.clone())?;
    let (_, hash, _, jobs, _, _) = parser.parse()?.into_run()?.into_parts();

    // Verify jobs parameter is valid
    assert!(jobs > 0, "Jobs should be a positive number");

    // The hash should be consistent
    assert!(!hash.is_empty(), "Hash should not be empty");
    assert_eq!(hash.len(), 8, "Hash should be 8 characters");

    // Parse again with same ottofile
    let mut parser2 = otto::cli::parser::Parser::new(args)?;
    let (_, hash2, _, jobs2, _, _) = parser2.parse()?.into_run()?.into_parts();

    assert_eq!(hash, hash2, "Hash should be consistent for same ottofile");
    assert_eq!(jobs, jobs2, "Jobs should be consistent for same arguments");

    temp_dir.close()?;
    Ok(())
}

#[tokio::test]
#[serial]
async fn test_workspace_metadata_structure() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Set up isolated test database
    setup_test_db(temp_path);

    let workspace = otto::executor::workspace::Workspace::new(temp_path.to_path_buf()).await?;
    workspace.init().await?;

    // Create and save execution context
    let mut execution_context = otto::executor::workspace::ExecutionContext::new();
    execution_context.ottofile = Some(PathBuf::from("/test/path/otto.yml"));
    execution_context.hash = "test1234".to_string();

    workspace.save_execution_context(execution_context).await?;

    // Verify the directory structure
    let run_dir = workspace.run();
    assert!(run_dir.exists(), "Run directory should exist");

    let run_yaml = run_dir.join("run.yaml");
    assert!(run_yaml.exists(), "run.yaml should exist in run directory");

    // Verify tasks directory exists
    let tasks_dir = run_dir.join("tasks");
    assert!(tasks_dir.exists(), "Tasks directory should exist in run directory");

    temp_dir.close()?;
    Ok(())
}
