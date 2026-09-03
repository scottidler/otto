use eyre::Result;
use serial_test::serial;
use std::collections::HashMap;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

use otto::Task;
use otto::cfg::config::ConfigSpec;
use otto::cfg::task::TaskSpec;
use otto::executor::scheduler::{TaskScheduler, TaskStatus};
use otto::executor::workspace::{ExecutionContext, Workspace};

// Test helper functions to reduce boilerplate
struct TestFixture {
    _temp_dir: TempDir, // Keep alive for cleanup
    temp_path: std::path::PathBuf,
    workspace: std::sync::Arc<Workspace>,
}

impl TestFixture {
    async fn new() -> Result<Self> {
        let temp_dir = TempDir::new()?;
        let temp_path = temp_dir.path().to_path_buf();

        // Set up isolated test database and workspace
        let db_path = temp_path.join("test_otto.db");
        let otto_home = temp_path.join(".otto");
        // SAFETY: This is safe in tests because we control the execution environment
        // and tests are isolated. The env var is set before any StateManager is created.
        unsafe {
            std::env::set_var("OTTO_DB_PATH", &db_path);
            std::env::set_var("OTTO_HOME", &otto_home);
        }

        let workspace = Workspace::new(temp_path.clone()).await?;
        workspace.init().await?;

        Ok(Self {
            _temp_dir: temp_dir,
            temp_path,
            workspace: std::sync::Arc::new(workspace),
        })
    }

    fn path(&self) -> &std::path::Path {
        &self.temp_path
    }

    fn file_path(&self, name: &str) -> std::path::PathBuf {
        self.temp_path.join(name)
    }

    fn write_file(&self, name: &str, content: &str) -> Result<()> {
        std::fs::write(self.file_path(name), content)?;
        Ok(())
    }

    fn create_dir(&self, name: &str) -> Result<()> {
        std::fs::create_dir_all(self.file_path(name))?;
        Ok(())
    }

    async fn create_scheduler(&self, tasks: Vec<Task>) -> Result<TaskScheduler> {
        TaskScheduler::new(tasks, self.workspace.clone(), ExecutionContext::new(), 2, false).await
    }

    fn create_task(&self, name: &str, action: &str, inputs: Vec<&str>, outputs: Vec<&str>) -> Task {
        Task::from_task_with_cwd(
            &TaskSpec {
                name: name.to_string(),
                action: action.to_string(),
                before: vec![],
                after: vec![],
                input: inputs.iter().map(|s| s.to_string()).collect(),
                output: outputs.iter().map(|s| s.to_string()).collect(),
                envs: HashMap::new(),
                params: otto::cfg::param::ParamSpecs::new(),
                help: None,
                foreach: None,
                virtual_parent: false,
                tty: None,
                on_failure: vec![],
            },
            &self.temp_path,
        )
        .expect("test fixture envs resolve")
    }

    fn create_task_with_deps(
        &self,
        name: &str,
        action: &str,
        inputs: Vec<&str>,
        outputs: Vec<&str>,
        before: Vec<&str>,
    ) -> Task {
        Task::from_task_with_cwd(
            &TaskSpec {
                name: name.to_string(),
                action: action.to_string(),
                before: before
                    .iter()
                    .map(|s| otto::cfg::edge::EdgeSpec::sugar(s.to_string()))
                    .collect(),
                after: vec![],
                input: inputs.iter().map(|s| s.to_string()).collect(),
                output: outputs.iter().map(|s| s.to_string()).collect(),
                envs: HashMap::new(),
                params: otto::cfg::param::ParamSpecs::new(),
                help: None,
                foreach: None,
                virtual_parent: false,
                tty: None,
                on_failure: vec![],
            },
            &self.temp_path,
        )
        .expect("test fixture envs resolve")
    }

    fn sleep_ms(ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_end_to_end_yaml() -> Result<()> {
    // Create a temporary directory for the test
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Set up isolated test database and workspace
    let db_path = temp_path.join("test_otto.db");
    let otto_home = temp_path.join(".otto");
    unsafe {
        std::env::set_var("OTTO_DB_PATH", &db_path);
        std::env::set_var("OTTO_HOME", &otto_home);
    }

    // Create source files
    std::fs::create_dir_all(temp_path.join("src"))?;
    std::fs::write(
        temp_path.join("src/main.c"),
        r#"
#include <stdio.h>
int main() {
    printf("Hello from file dependencies!\n");
    return 0;
}
"#,
    )?;
    std::fs::write(
        temp_path.join("src/utils.c"),
        r#"
#include <stdio.h>
void utils() {
    printf("Utils function\n");
}
"#,
    )?;
    std::fs::write(temp_path.join("Makefile"), "all:\n\techo 'Building...'\n")?;

    // Change to temp directory
    let original_dir = std::env::current_dir()?;
    std::env::set_current_dir(temp_path)?;

    // Create YAML configuration with file dependencies
    let yaml_content = r#"
otto:
  name: "File Dependencies Integration Test"
  api: 1
  jobs: 2

tasks:
  compile:
    input:
      - "src/*.c"
      - "Makefile"
    output:
      - "build/app"
    action: |
      #!/bin/bash
      mkdir -p build
      echo "Compiling C files..."
      gcc -o build/app src/*.c
      echo "Compilation complete"
    help: "Compile C application with file dependencies"

  test:
    before: ["compile"]
    input:
      - "build/app"
    output:
      - "test_results.log"
    action: |
      #!/bin/bash
      echo "Running tests..."
      if [ -f build/app ]; then
        echo "Test: Binary exists - PASS" > test_results.log
        ./build/app >> test_results.log 2>&1
        echo "Test: Binary execution - PASS" >> test_results.log
      else
        echo "Test: Binary missing - FAIL" > test_results.log
        exit 1
      fi
    help: "Run tests on compiled application"

  package:
    before: ["test"]
    input:
      - "build/app"
      - "test_results.log"
    output:
      - "dist/package.tar.gz"
    action: |
      #!/bin/bash
      echo "Creating package..."
      mkdir -p dist
      tar -czf dist/package.tar.gz build/app test_results.log
      echo "Package created successfully"
    help: "Package application and test results"
"#;

    let config_spec: ConfigSpec = serde_yaml_ng::from_str(yaml_content)?;

    // Convert tasks to TaskSpecs
    let mut task_specs = Vec::new();
    for task_spec in config_spec.tasks.values() {
        let spec = Task::from_task_with_cwd(task_spec, temp_path).expect("test fixture envs resolve");
        task_specs.push(spec);
    }

    // Create workspace and scheduler
    let workspace = Workspace::new(temp_path.to_path_buf()).await?;
    workspace.init().await?;
    let _scheduler = TaskScheduler::new(
        task_specs.clone(),
        std::sync::Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    // Verify file dependencies were parsed correctly
    let compile_spec = task_specs.iter().find(|t| t.name == "compile").unwrap();
    println!("DEBUG: compile_spec.file_deps = {:?}", compile_spec.file_deps);
    println!("DEBUG: compile_spec.file_deps.len() = {}", compile_spec.file_deps.len());
    assert!(compile_spec.file_deps.len() >= 3); // main.c, utils.c, Makefile
    assert!(compile_spec.file_deps.iter().any(|f| f.contains("main.c")));
    assert!(compile_spec.file_deps.iter().any(|f| f.contains("utils.c")));
    assert!(compile_spec.file_deps.iter().any(|f| f.contains("Makefile")));
    assert_eq!(compile_spec.output_deps.len(), 1);
    assert!(compile_spec.output_deps.iter().any(|f| f.contains("build/app")));

    let test_spec = task_specs.iter().find(|t| t.name == "test").unwrap();
    assert_eq!(test_spec.task_deps.len(), 1);
    assert_eq!(test_spec.task_deps[0].task, "compile");
    assert!(test_spec.file_deps.iter().any(|f| f.contains("build/app")));
    assert!(test_spec.output_deps.iter().any(|f| f.contains("test_results.log")));

    println!("DEBUG: All file dependency assertions passed!");

    // EARLY RETURN FOR NOW - just test file dependency parsing
    std::env::set_current_dir(original_dir)?;
    return Ok(());
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_glob_patterns() -> Result<()> {
    let fixture = TestFixture::new().await?;

    // Create complex directory structure using helper
    fixture.create_dir("src/lib")?;
    fixture.create_dir("tests")?;
    fixture.create_dir("docs")?;

    // Create various files using helper
    fixture.write_file("src/main.rs", "fn main() {}")?;
    fixture.write_file("src/lib.rs", "pub mod lib;")?;
    fixture.write_file("src/lib/utils.rs", "pub fn utils() {}")?;
    fixture.write_file("tests/test1.rs", "#[test] fn test1() {}")?;
    fixture.write_file("tests/test2.rs", "#[test] fn test2() {}")?;
    fixture.write_file("docs/README.md", "# Documentation")?;
    fixture.write_file("Cargo.toml", "[package]")?;

    let task = fixture.create_task(
        "build_all",
        "echo 'Building all Rust files'",
        vec![
            "src/**/*.rs", // Recursive glob
            "tests/*.rs",  // Simple glob
            "*.toml",      // Root-level glob
            "docs/*.md",   // Documentation glob
        ],
        vec!["target/debug/app"],
    );

    // Verify glob patterns were resolved
    assert!(task.file_deps.len() >= 6); // main.rs, lib.rs, utils.rs, test1.rs, test2.rs, Cargo.toml, README.md
    assert!(task.file_deps.iter().any(|f| f.contains("main.rs")));
    assert!(task.file_deps.iter().any(|f| f.contains("lib.rs")));
    assert!(task.file_deps.iter().any(|f| f.contains("utils.rs")));
    assert!(task.file_deps.iter().any(|f| f.contains("test1.rs")));
    assert!(task.file_deps.iter().any(|f| f.contains("test2.rs")));
    assert!(task.file_deps.iter().any(|f| f.contains("Cargo.toml")));
    assert!(task.file_deps.iter().any(|f| f.contains("README.md")));

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_error_handling() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Set up isolated test database and workspace
    let db_path = temp_path.join("test_otto.db");
    let otto_home = temp_path.join(".otto");
    unsafe {
        std::env::set_var("OTTO_DB_PATH", &db_path);
        std::env::set_var("OTTO_HOME", &otto_home);
    }

    // Create task with missing input files
    let missing_input = temp_path.join("nonexistent.txt");
    let output_file = temp_path.join("output.txt");

    let task = Task::from_task_with_cwd(
        &TaskSpec {
            name: "fail_task".to_string(),
            action: "echo 'This will run because input is missing'".to_string(),
            before: vec![],
            after: vec![],
            input: vec![missing_input.to_string_lossy().to_string()],
            output: vec![output_file.to_string_lossy().to_string()],
            envs: HashMap::new(),
            params: otto::cfg::param::ParamSpecs::new(),
            help: None,
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        },
        temp_path,
    )
    .expect("test fixture envs resolve");

    let workspace = Workspace::new(temp_path.to_path_buf()).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![task.clone()],
        std::sync::Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    // Task should need to run when input file doesn't exist (conservative approach)
    let needs_rebuild = scheduler.needs_rebuild(&task).await;
    assert!(needs_rebuild, "Task should need to run when input file is missing");

    // Create task with read-only directory output (should handle permission errors gracefully)
    let readonly_dir = temp_path.join("readonly");
    std::fs::create_dir_all(&readonly_dir)?;

    // Make directory read-only (Unix-specific)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&readonly_dir)?.permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&readonly_dir, perms)?;
    }

    let readonly_output = readonly_dir.join("output.txt");
    let task_readonly = Task::from_task_with_cwd(
        &TaskSpec {
            name: "readonly_task".to_string(),
            action: format!("echo 'test' > {}", readonly_output.display()).to_string(),
            before: vec![],
            after: vec![],
            input: vec![],
            output: vec![readonly_output.to_string_lossy().to_string()],
            envs: HashMap::new(),
            params: otto::cfg::param::ParamSpecs::new(),
            help: None,
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        },
        temp_path,
    )
    .expect("test fixture envs resolve");

    // Should handle permission issues gracefully
    let needs_rebuild_readonly = scheduler.needs_rebuild(&task_readonly).await;
    // Should need to run since output doesn't exist
    assert!(needs_rebuild_readonly);

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_performance() -> Result<()> {
    let fixture = TestFixture::new().await?;

    // Create many files to test performance
    let num_files = 1000;
    let mut input_files = Vec::new();

    for i in 0..num_files {
        let filename = format!("input_{i:04}.txt");
        fixture.write_file(&filename, &format!("content {i}"))?;
        input_files.push(filename);
    }

    let task = fixture.create_task(
        "perf_test",
        &format!(
            "find {} -name 'input_*.txt' | wc -l > {}",
            fixture.path().display(),
            fixture.file_path("combined.txt").display()
        ),
        input_files.iter().map(|s| s.as_str()).collect(),
        vec!["combined.txt"],
    );

    let scheduler = fixture.create_scheduler(vec![task.clone()]).await?;

    // Measure file dependency checking performance
    let start = std::time::Instant::now();
    let needs_rebuild = scheduler.needs_rebuild(&task).await;
    let duration = start.elapsed();

    assert!(needs_rebuild, "Task should need to run initially");
    assert!(
        duration.as_millis() < 2000,
        "File dependency checking should be fast with {} files (took {}ms)",
        num_files,
        duration.as_millis()
    );

    println!("File dependency check for {num_files} files took: {duration:?}");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_mixed_task_and_file_dependencies() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Set up isolated test database and workspace
    let db_path = temp_path.join("test_otto.db");
    let otto_home = temp_path.join(".otto");
    unsafe {
        std::env::set_var("OTTO_DB_PATH", &db_path);
        std::env::set_var("OTTO_HOME", &otto_home);
    }

    // Create source files
    let config_file = temp_path.join("config.json");
    let data_file = temp_path.join("data.csv");
    let processed_file = temp_path.join("processed.json");
    let report_file = temp_path.join("report.html");

    std::fs::write(&config_file, r#"{"version": "1.0", "format": "json"}"#)?;
    std::fs::write(&data_file, "id,name,value\n1,test,100\n2,example,200\n")?;

    let preprocess_task = Task::from_task_with_cwd(
        &TaskSpec {
            name: "preprocess".to_string(),
            action: format!(
                r#"echo '{{"status": "processed", "config": ' > {}; cat {} >> {}; echo '}}' >> {}"#,
                processed_file.display(),
                config_file.display(),
                processed_file.display(),
                processed_file.display()
            )
            .to_string(),
            before: vec![],
            after: vec![],
            input: vec![
                config_file.to_string_lossy().to_string(),
                data_file.to_string_lossy().to_string(),
            ],
            output: vec![processed_file.to_string_lossy().to_string()],
            envs: HashMap::new(),
            params: otto::cfg::param::ParamSpecs::new(),
            help: None,
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        },
        temp_path,
    )
    .expect("test fixture envs resolve");

    let analyze_task = Task::from_task_with_cwd(
        &TaskSpec {
            name: "analyze".to_string(),
            action: format!(
                "echo '<html><body>Analysis complete: ' > {}; cat {} >> {}; echo '</body></html>' >> {}",
                report_file.display(),
                processed_file.display(),
                report_file.display(),
                report_file.display()
            )
            .to_string(),
            before: vec![otto::cfg::edge::EdgeSpec::sugar("preprocess")],
            after: vec![],
            input: vec![processed_file.to_string_lossy().to_string()],
            output: vec![report_file.to_string_lossy().to_string()],
            envs: HashMap::new(),
            params: otto::cfg::param::ParamSpecs::new(),
            help: None,
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        },
        temp_path,
    )
    .expect("test fixture envs resolve");

    let workspace = Workspace::new(temp_path.to_path_buf()).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![preprocess_task.clone(), analyze_task.clone()],
        std::sync::Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    // Both tasks should need to run initially
    assert!(scheduler.needs_rebuild(&preprocess_task).await);
    assert!(scheduler.needs_rebuild(&analyze_task).await);

    // Execute all tasks
    timeout(Duration::from_secs(10), scheduler.execute_all()).await??;

    // Verify completion and output files
    assert_eq!(scheduler.get_task_status("preprocess").await, TaskStatus::Completed);
    assert_eq!(scheduler.get_task_status("analyze").await, TaskStatus::Completed);
    assert!(processed_file.exists());
    assert!(report_file.exists());

    // Neither task should need to rebuild
    assert!(!scheduler.needs_rebuild(&preprocess_task).await);
    assert!(!scheduler.needs_rebuild(&analyze_task).await);

    // Modify input file
    std::thread::sleep(std::time::Duration::from_millis(100));
    std::fs::write(&data_file, "id,name,value\n1,test,150\n2,example,250\n3,new,300\n")?;

    // Preprocess should need rebuild due to file dependency
    assert!(scheduler.needs_rebuild(&preprocess_task).await);
    // Analyze might or might not need rebuild depending on when we check
    // (it depends on preprocess completing and updating the processed file)

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_incremental_detection() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Set up isolated test database and workspace
    let db_path = temp_path.join("test_otto.db");
    let otto_home = temp_path.join(".otto");
    unsafe {
        std::env::set_var("OTTO_DB_PATH", &db_path);
        std::env::set_var("OTTO_HOME", &otto_home);
    }

    // Create input files
    let input1 = temp_path.join("main.c");
    let input2 = temp_path.join("main.h");
    std::fs::write(&input1, "int main() { return 0; }")?;
    std::fs::write(&input2, "#ifndef MAIN_H\n#define MAIN_H\n#endif")?;

    // Create output file
    let output = temp_path.join("main.o");

    let task = Task::from_task_with_cwd(
        &TaskSpec {
            name: "compile".to_string(),
            action: "gcc -c main.c -o main.o".to_string(),
            before: vec![],
            after: vec![],
            input: vec!["main.c".to_string(), "main.h".to_string()],
            output: vec!["main.o".to_string()],
            envs: HashMap::new(),
            params: otto::cfg::param::ParamSpecs::new(),
            help: None,
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        },
        temp_path,
    )
    .expect("test fixture envs resolve");

    let workspace = Workspace::new(temp_path.to_path_buf()).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![task.clone()],
        std::sync::Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    // Test 1: No output file exists - should need rebuild
    assert!(
        scheduler.needs_rebuild(&task).await,
        "Should need rebuild when output missing"
    );

    // Test 2: Create output file newer than inputs - should NOT need rebuild
    std::thread::sleep(std::time::Duration::from_millis(10)); // Ensure time difference
    std::fs::write(&output, "compiled object")?;
    assert!(
        !scheduler.needs_rebuild(&task).await,
        "Should NOT need rebuild when output newer than inputs"
    );

    // Test 3: Touch input file to make it newer - should need rebuild
    std::thread::sleep(std::time::Duration::from_millis(10)); // Ensure time difference
    std::fs::write(&input1, "int main() { return 1; }")?; // Modify input
    assert!(
        scheduler.needs_rebuild(&task).await,
        "Should need rebuild when input newer than output"
    );

    // Test 4: Update output to be newer again - should NOT need rebuild
    std::thread::sleep(std::time::Duration::from_millis(10)); // Ensure time difference
    std::fs::write(&output, "recompiled object")?;
    assert!(
        !scheduler.needs_rebuild(&task).await,
        "Should NOT need rebuild when output updated"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_with_real_execution() -> Result<()> {
    let fixture = TestFixture::new().await?;

    // Create input file with known content
    fixture.write_file("source.txt", "Hello, File Dependencies!")?;

    let task = fixture.create_task(
        "copy_task",
        &format!(
            "cp {} {}",
            fixture.file_path("source.txt").display(),
            fixture.file_path("result.txt").display()
        ),
        vec!["source.txt"],
        vec!["result.txt"],
    );

    let scheduler = fixture.create_scheduler(vec![task.clone()]).await?;

    // Task should need to run initially (output doesn't exist)
    assert!(
        scheduler.needs_rebuild(&task).await,
        "Should need rebuild when output missing"
    );

    // Execute the task
    timeout(Duration::from_secs(10), scheduler.execute_all()).await??;

    // Verify task completed and output file was created
    assert_eq!(scheduler.get_task_status("copy_task").await, TaskStatus::Completed);
    assert!(
        fixture.file_path("result.txt").exists(),
        "Output file should exist after execution"
    );

    let output_content = std::fs::read_to_string(fixture.file_path("result.txt"))?;
    assert_eq!(output_content, "Hello, File Dependencies!", "Output should match input");

    // Task should NOT need to run again (outputs are up-to-date)
    assert!(
        !scheduler.needs_rebuild(&task).await,
        "Should not need rebuild after successful execution"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_multiple_files() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Set up isolated test database and workspace
    let db_path = temp_path.join("test_otto.db");
    let otto_home = temp_path.join(".otto");
    unsafe {
        std::env::set_var("OTTO_DB_PATH", &db_path);
        std::env::set_var("OTTO_HOME", &otto_home);
    }

    // Create multiple input files with known content
    let input1 = temp_path.join("file1.txt");
    let input2 = temp_path.join("file2.txt");
    let input3 = temp_path.join("file3.txt");
    let output1 = temp_path.join("combined.txt");
    let output2 = temp_path.join("summary.txt");

    std::fs::write(&input1, "Line 1")?;
    std::fs::write(&input2, "Line 2")?;
    std::fs::write(&input3, "Line 3")?;

    let task = Task::from_task_with_cwd(
        &TaskSpec {
            name: "combine".to_string(),
            action: format!(
                "cat {} {} {} > {} && echo 'Files: 3' > {}",
                input1.display(),
                input2.display(),
                input3.display(),
                output1.display(),
                output2.display()
            ),
            before: vec![],
            after: vec![],
            input: vec![
                "file1.txt".to_string(),
                "file2.txt".to_string(),
                "file3.txt".to_string(),
            ],
            output: vec!["combined.txt".to_string(), "summary.txt".to_string()],
            envs: HashMap::new(),
            params: otto::cfg::param::ParamSpecs::new(),
            help: None,
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        },
        temp_path,
    )
    .expect("test fixture envs resolve");

    let workspace = Workspace::new(temp_path.to_path_buf()).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![task.clone()],
        std::sync::Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    // Should need to run - no outputs exist
    assert!(
        scheduler.needs_rebuild(&task).await,
        "Should need rebuild with missing outputs"
    );

    // Execute task
    timeout(Duration::from_secs(10), scheduler.execute_all()).await??;

    // Verify outputs were created
    assert!(output1.exists(), "combined.txt should exist");
    assert!(output2.exists(), "summary.txt should exist");
    assert_eq!(std::fs::read_to_string(&output1)?, "Line 1Line 2Line 3");
    assert_eq!(std::fs::read_to_string(&output2)?, "Files: 3\n");

    // Should NOT need rebuild - outputs are newer than inputs
    assert!(
        !scheduler.needs_rebuild(&task).await,
        "Should not need rebuild after execution"
    );

    // Modify one input file to trigger rebuild
    std::thread::sleep(std::time::Duration::from_millis(10)); // Ensure time difference
    std::fs::write(&input2, "Modified Line 2")?;

    // Should need rebuild - input is newer than output
    assert!(
        scheduler.needs_rebuild(&task).await,
        "Should need rebuild when input is modified"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_task_chain() -> Result<()> {
    let fixture = TestFixture::new().await?;

    // Create initial input file
    fixture.write_file("source.md", "# Hello\nThis is markdown.")?;

    // Task 1: Process markdown to text
    let process_task = fixture.create_task(
        "process",
        &format!(
            "sed 's/#//g' {} | tr -d '*' > {}",
            fixture.file_path("source.md").display(),
            fixture.file_path("processed.txt").display()
        ),
        vec!["source.md"],
        vec!["processed.txt"],
    );

    // Task 2: Convert text to HTML (depends on task 1)
    let convert_task = fixture.create_task_with_deps(
        "convert",
        &format!(
            "echo '<html><body><pre>' > {} && cat {} >> {} && echo '</pre></body></html>' >> {}",
            fixture.file_path("final.html").display(),
            fixture.file_path("processed.txt").display(),
            fixture.file_path("final.html").display(),
            fixture.file_path("final.html").display()
        ),
        vec!["processed.txt"],
        vec!["final.html"],
        vec!["process"], // Before dependency
    );

    let scheduler = fixture
        .create_scheduler(vec![process_task.clone(), convert_task.clone()])
        .await?;

    // Both tasks should need to run initially
    assert!(
        scheduler.needs_rebuild(&process_task).await,
        "Process task should need rebuild"
    );
    assert!(
        scheduler.needs_rebuild(&convert_task).await,
        "Convert task should need rebuild"
    );

    // Execute all tasks
    timeout(Duration::from_secs(10), scheduler.execute_all()).await??;

    // Verify both tasks completed
    assert_eq!(scheduler.get_task_status("process").await, TaskStatus::Completed);
    assert_eq!(scheduler.get_task_status("convert").await, TaskStatus::Completed);

    // Verify files were created with expected content
    assert!(
        fixture.file_path("processed.txt").exists(),
        "Intermediate file should exist"
    );
    assert!(fixture.file_path("final.html").exists(), "Final output should exist");

    let final_content = std::fs::read_to_string(fixture.file_path("final.html"))?;
    assert!(final_content.contains("<html>"), "Final output should be HTML");
    assert!(
        final_content.contains("Hello"),
        "Final output should contain processed content"
    );

    // Neither task should need rebuild now
    assert!(
        !scheduler.needs_rebuild(&process_task).await,
        "Process task should not need rebuild after completion"
    );
    assert!(
        !scheduler.needs_rebuild(&convert_task).await,
        "Convert task should not need rebuild after completion"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_task_skipping() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Set up isolated test database and workspace
    let db_path = temp_path.join("test_otto.db");
    let otto_home = temp_path.join(".otto");
    unsafe {
        std::env::set_var("OTTO_DB_PATH", &db_path);
        std::env::set_var("OTTO_HOME", &otto_home);
    }

    // Create input and output files where output is already newer
    let input_file = temp_path.join("config.txt");
    let output_file = temp_path.join("generated.json");

    std::fs::write(&input_file, "setting=value")?;

    // Wait to ensure timestamp difference
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Create output file that's newer than input
    std::fs::write(&output_file, r#"{"setting": "value"}"#)?;

    let task = Task::from_task_with_cwd(
        &TaskSpec {
            name: "generate".to_string(),
            action: "echo 'This should not run!' && exit 1".to_string(), // Will fail if executed
            before: vec![],
            after: vec![],
            input: vec!["config.txt".to_string()],
            output: vec!["generated.json".to_string()],
            envs: HashMap::new(),
            params: otto::cfg::param::ParamSpecs::new(),
            help: None,
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        },
        temp_path,
    )
    .expect("test fixture envs resolve");

    let workspace = Workspace::new(temp_path.to_path_buf()).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![task.clone()],
        std::sync::Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    // Task should NOT need to run - output is newer than input
    assert!(
        !scheduler.needs_rebuild(&task).await,
        "Task should not need rebuild when output is newer"
    );

    // Verify the output file still contains original content (no execution needed)
    let output_content = std::fs::read_to_string(&output_file)?;
    assert_eq!(
        output_content, r#"{"setting": "value"}"#,
        "Output should be unchanged when task doesn't need rebuild"
    );

    // Test that modifying input triggers rebuild
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(&input_file, "setting=new_value")?;
    assert!(
        scheduler.needs_rebuild(&task).await,
        "Task should need rebuild when input is modified"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_modification_detection() -> Result<()> {
    let fixture = TestFixture::new().await?;

    // Create files in chronological order with explicit delays
    fixture.write_file("old_config.txt", "old_value=1")?;
    TestFixture::sleep_ms(10);

    fixture.write_file("result.txt", "processed output")?;
    TestFixture::sleep_ms(10);

    fixture.write_file("new_config.txt", "new_value=2")?;

    // Task with mixed old/new inputs
    let task = Task::from_task_with_cwd(
        &TaskSpec {
            name: "process_configs".to_string(),
            action: "echo 'processing'".to_string(),
            before: vec![],
            after: vec![],
            input: vec!["old_config.txt".to_string(), "new_config.txt".to_string()],
            output: vec!["result.txt".to_string()],
            envs: HashMap::new(),
            params: otto::cfg::param::ParamSpecs::new(),
            help: None,
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        },
        &fixture.temp_path,
    )
    .expect("test fixture envs resolve");

    let scheduler = fixture.create_scheduler(vec![task.clone()]).await?;

    // Should need rebuild because new_file is newer than output
    assert!(
        scheduler.needs_rebuild(&task).await,
        "Should need rebuild when any input is newer than output"
    );

    // Update output to be newer than all inputs
    TestFixture::sleep_ms(10);
    fixture.write_file("result.txt", "updated output")?;

    // Should NOT need rebuild now
    assert!(
        !scheduler.needs_rebuild(&task).await,
        "Should not need rebuild when output is newer than all inputs"
    );

    // Touch the old file to make it newer
    TestFixture::sleep_ms(10);
    fixture.write_file("old_config.txt", "old_value=1")?; // Same content, newer timestamp

    // Should need rebuild again
    assert!(
        scheduler.needs_rebuild(&task).await,
        "Should need rebuild when any input file is touched"
    );

    Ok(())
}
