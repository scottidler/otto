use otto::cli::parser::Parser;
use serial_test::serial;
use std::fs;
use tempfile::TempDir;

#[test]
#[serial]
fn test_boolean_flags_integration() {
    let temp_dir = TempDir::new().unwrap();
    let otto_file = temp_dir.path().join("otto.yml");

    let config = r#"
otto:
  api: 1
  tasks: [build]

tasks:
  build:
    params:
      -v|--verbose:
        default: false
        help: Enable verbose output
      -f|--force:
        default: false
        help: Force rebuild
      --dry-run:
        default: false
        help: Show what would be done
    action: |
      #!/bin/bash
      echo "Verbose: ${verbose}"
      echo "Force: ${force}"
      echo "Dry run: ${dry_run}"
    "#;

    fs::write(&otto_file, config).unwrap();

    // Test with flags present
    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "build".to_string(),
        "--verbose".to_string(),
        "--force".to_string(),
        "--dry-run".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let (tasks, _, _, _, _, _) = parser.parse().unwrap().into_run().unwrap().into_parts();

    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.name, "build");
    assert_eq!(task.envs.get("verbose").unwrap(), "true");
    assert_eq!(task.envs.get("force").unwrap(), "true");
    assert_eq!(task.envs.get("dry_run").unwrap(), "true");
}

#[test]
#[serial]
fn test_boolean_flags_absent_integration() {
    let temp_dir = TempDir::new().unwrap();
    let otto_file = temp_dir.path().join("otto.yml");

    let config = r#"
otto:
  api: 1
  tasks: [build]

tasks:
  build:
    params:
      -v|--verbose:
        default: false
        help: Enable verbose output
      -f|--force:
        default: false
        help: Force rebuild
    action: |
      #!/bin/bash
      echo "Verbose: ${verbose}"
      echo "Force: ${force}"
    "#;

    fs::write(&otto_file, config).unwrap();

    // Test with no flags (should use defaults)
    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "build".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let (tasks, _, _, _, _, _) = parser.parse().unwrap().into_run().unwrap().into_parts();

    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.name, "build");
    assert_eq!(task.envs.get("verbose").unwrap(), "false");
    assert_eq!(task.envs.get("force").unwrap(), "false");
}

#[test]
#[serial]
fn test_argument_flags_integration() {
    let temp_dir = TempDir::new().unwrap();
    let otto_file = temp_dir.path().join("otto.yml");

    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  deploy:
    params:
      -e|--env:
        default: development
        choices: [development, staging, production]
        help: Target environment
      --timeout:
        default: 30
        help: Timeout in seconds
      -p|--port:
        default: 8080
        help: Port number
    action: |
      #!/bin/bash
      echo "Environment: ${env}"
      echo "Timeout: ${timeout}"
      echo "Port: ${port}"
    "#;

    fs::write(&otto_file, config).unwrap();

    // Test with explicit values
    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "deploy".to_string(),
        "--env".to_string(),
        "production".to_string(),
        "--timeout".to_string(),
        "60".to_string(),
        "-p".to_string(),
        "3000".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let (tasks, _, _, _, _, _) = parser.parse().unwrap().into_run().unwrap().into_parts();

    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.name, "deploy");
    assert_eq!(task.envs.get("env").unwrap(), "production");
    assert_eq!(task.envs.get("timeout").unwrap(), "60");
    assert_eq!(task.envs.get("port").unwrap(), "3000");
}

#[test]
#[serial]
fn test_argument_flags_with_defaults_integration() {
    let temp_dir = TempDir::new().unwrap();
    let otto_file = temp_dir.path().join("otto.yml");

    let config = r#"
otto:
  api: 1
  tasks: [serve]

tasks:
  serve:
    params:
      -p|--port:
        default: 3000
        help: Port number
      --host:
        default: localhost
        help: Host address
      -w|--workers:
        default: 4
        help: Number of workers
    action: |
      #!/bin/bash
      echo "Port: ${port}"
      echo "Host: ${host}"
      echo "Workers: ${workers}"
    "#;

    fs::write(&otto_file, config).unwrap();

    // Test with some explicit values and some defaults
    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "serve".to_string(),
        "--port".to_string(),
        "8080".to_string(),
        // host and workers should use defaults
    ];

    let mut parser = Parser::new(args).unwrap();
    let (tasks, _, _, _, _, _) = parser.parse().unwrap().into_run().unwrap().into_parts();

    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.name, "serve");
    assert_eq!(task.envs.get("port").unwrap(), "8080");
    assert_eq!(task.envs.get("host").unwrap(), "localhost");
    assert_eq!(task.envs.get("workers").unwrap(), "4");
}

#[test]
#[serial]
fn test_mixed_flags_integration() {
    let temp_dir = TempDir::new().unwrap();
    let otto_file = temp_dir.path().join("otto.yml");

    let config = r#"
otto:
  api: 1
  tasks: [test]

tasks:
  test:
    params:
      # Boolean flags
      -v|--verbose:
        default: false
        help: Enable verbose output
      --coverage:
        default: false
        help: Generate coverage report
      --watch:
        default: false
        help: Watch for file changes

      # Argument flags
      -p|--pattern:
        default: "**/*.test.js"
        help: Test file pattern
      --reporter:
        choices: [spec, json, junit, tap]
        default: spec
        help: Test reporter format
      --timeout:
        default: 5000
        help: Test timeout in milliseconds
    action: |
      #!/bin/bash
      echo "Verbose: ${verbose}"
      echo "Coverage: ${coverage}"
      echo "Watch: ${watch}"
      echo "Pattern: ${pattern}"
      echo "Reporter: ${reporter}"
      echo "Timeout: ${timeout}"
    "#;

    fs::write(&otto_file, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "test".to_string(),
        "--verbose".to_string(),
        "--coverage".to_string(),
        "--pattern".to_string(),
        "src/**/*.test.js".to_string(),
        "--reporter".to_string(),
        "json".to_string(),
        "--timeout".to_string(),
        "10000".to_string(),
        // watch should default to false
    ];

    let mut parser = Parser::new(args).unwrap();
    let (tasks, _, _, _, _, _) = parser.parse().unwrap().into_run().unwrap().into_parts();

    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.name, "test");

    // Boolean flags
    assert_eq!(task.envs.get("verbose").unwrap(), "true");
    assert_eq!(task.envs.get("coverage").unwrap(), "true");
    assert_eq!(task.envs.get("watch").unwrap(), "false");

    // Argument flags
    assert_eq!(task.envs.get("pattern").unwrap(), "src/**/*.test.js");
    assert_eq!(task.envs.get("reporter").unwrap(), "json");
    assert_eq!(task.envs.get("timeout").unwrap(), "10000");
}

#[test]
#[serial]
fn test_short_flag_combinations() {
    let temp_dir = TempDir::new().unwrap();
    let otto_file = temp_dir.path().join("otto.yml");

    let config = r#"
otto:
  api: 1
  tasks: [build]

tasks:
  build:
    params:
      -v|--verbose:
        default: false
        help: Enable verbose output
      -f|--force:
        default: false
        help: Force rebuild
      -q|--quiet:
        default: false
        help: Quiet output
      -e|--env:
        default: development
        choices: [development, staging, production]
        help: Target environment
    action: |
      #!/bin/bash
      echo "Building..."
    "#;

    fs::write(&otto_file, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "build".to_string(),
        "-v".to_string(),
        "-f".to_string(),
        "-e".to_string(),
        "production".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let (tasks, _, _, _, _, _) = parser.parse().unwrap().into_run().unwrap().into_parts();

    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.name, "build");
    assert_eq!(task.envs.get("verbose").unwrap(), "true");
    assert_eq!(task.envs.get("force").unwrap(), "true");
    assert_eq!(task.envs.get("quiet").unwrap(), "false");
    assert_eq!(task.envs.get("env").unwrap(), "production");
}

#[test]
#[serial]
fn test_serial_flag_chains_foreach_subtasks() {
    let temp_dir = TempDir::new().unwrap();
    let otto_file = temp_dir.path().join("otto.yml");

    let config = r#"
otto:
  api: 1
  tasks: [examples]

tasks:
  examples:
    help: Run examples
    foreach:
      items: [a, b, c, d]
      as: item
    bash: echo "${item}"
    "#;

    fs::write(&otto_file, config).unwrap();

    // Without --Serial, subtasks should run in parallel (no dependencies between them)
    let args_parallel = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "examples".to_string(),
    ];

    let mut parser = Parser::new(args_parallel).unwrap();
    let (tasks_parallel, _, _, _, _, _) = parser.parse().unwrap().into_run().unwrap().into_parts();

    // Subtasks should not depend on each other (parent's When::Always edges to subtasks
    // are excluded - that's the aggregation gate, not a sibling dependency).
    for task in &tasks_parallel {
        if !task.name.contains(':') {
            continue;
        }
        for dep in &task.task_deps {
            assert!(
                !dep.task.starts_with("examples:"),
                "Parallel subtasks should not depend on each other"
            );
        }
    }

    // Parallel subtasks join no serial group either.
    for task in tasks_parallel.iter().filter(|t| t.name.starts_with("examples:")) {
        assert_eq!(task.serial_group, None, "{} should carry no ordering", task.name);
    }

    // With --Serial, subtasks join one ordered group. Rewritten in Phase 4 of
    // docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md: ordering used to be
    // sibling `before:` edges, which made `otto examples:d` drag in a, b and c.
    let args_serial = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "examples".to_string(),
        "--Serial".to_string(),
    ];

    let mut parser = Parser::new(args_serial).unwrap();
    let (tasks_serial, _, _, _, _, _) = parser.parse().unwrap().into_run().unwrap().into_parts();

    let mut ordered: Vec<(usize, String)> = Vec::new();
    for task in tasks_serial.iter().filter(|t| t.name.starts_with("examples:")) {
        assert_eq!(
            task.serial_group.as_deref(),
            Some("examples"),
            "{} should be in serial group 'examples'",
            task.name
        );
        assert!(
            !task.task_deps.iter().any(|d| d.task.starts_with("examples:")),
            "Serial ordering must not be a sibling edge, got: {:?}",
            task.task_deps
        );
        ordered.push((task.serial_index, task.name.clone()));
    }

    ordered.sort();
    assert_eq!(
        ordered,
        vec![
            (0, "examples:a".to_string()),
            (1, "examples:b".to_string()),
            (2, "examples:c".to_string()),
            (3, "examples:d".to_string()),
        ],
        "Serial subtasks should be indexed in declared foreach order"
    );
}

#[test]
#[serial]
fn test_foreach_glob_resolves_relative_to_ottofile_not_cwd() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    // Create directory structure:
    // /root/
    //   otto.yml
    //   examples/
    //     foo.rs
    //     bar.rs
    //   subdir/
    //     (this is where we'll run from)
    let examples_dir = root.join("examples");
    fs::create_dir(&examples_dir).unwrap();
    fs::write(examples_dir.join("foo.rs"), "fn main() {}").unwrap();
    fs::write(examples_dir.join("bar.rs"), "fn main() {}").unwrap();

    let subdir = root.join("subdir");
    fs::create_dir(&subdir).unwrap();

    let otto_file = root.join("otto.yml");

    let config = r#"
otto:
  api: 1
  tasks: [examples]

tasks:
  examples:
    help: Run examples
    foreach:
      glob: "examples/*.rs"
      as: example
    bash: echo "${example}"
    "#;

    fs::write(&otto_file, config).unwrap();

    // Change to subdir and run with explicit ottofile path
    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "examples".to_string(),
    ];

    // Save current dir, change to subdir, then restore
    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(&subdir).unwrap();

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse();

    // Restore original directory
    std::env::set_current_dir(&original_dir).unwrap();

    // The parse should succeed and find the examples
    let (tasks, _, _, _, _, _) = result
        .map(|o| o.into_run().unwrap().into_parts())
        .expect("Parse should succeed even from subdirectory");

    // Should have expanded subtasks for both .rs files
    let subtask_names: Vec<_> = tasks.iter().map(|t| t.name.as_str()).collect();

    assert!(
        subtask_names.iter().any(|n| n.starts_with("examples:")),
        "Should have foreach subtasks, got: {:?}",
        subtask_names
    );

    // Should have exactly 2 subtasks (foo.rs and bar.rs)
    let foreach_subtasks: Vec<_> = tasks
        .iter()
        .filter(|t| t.name.starts_with("examples:") && t.name != "examples")
        .collect();

    assert_eq!(
        foreach_subtasks.len(),
        2,
        "Should have 2 foreach subtasks for foo.rs and bar.rs, got: {:?}",
        foreach_subtasks.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
}

#[test]
#[serial]
fn test_foreach_glob_works_from_project_subdirectory() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    // Create a more realistic project structure:
    // /project/
    //   otto.yml
    //   scripts/
    //     build.sh
    //     test.sh
    //     deploy.sh
    //   docs/
    //     (run from here)
    //   src/
    //     main.rs
    let scripts_dir = root.join("scripts");
    fs::create_dir(&scripts_dir).unwrap();
    fs::write(scripts_dir.join("build.sh"), "#!/bin/bash\necho build").unwrap();
    fs::write(scripts_dir.join("test.sh"), "#!/bin/bash\necho test").unwrap();
    fs::write(scripts_dir.join("deploy.sh"), "#!/bin/bash\necho deploy").unwrap();

    let docs_dir = root.join("docs");
    fs::create_dir(&docs_dir).unwrap();

    let src_dir = root.join("src");
    fs::create_dir(&src_dir).unwrap();
    fs::write(src_dir.join("main.rs"), "fn main() {}").unwrap();

    let otto_file = root.join("otto.yml");

    let config = r#"
otto:
  api: 1
  tasks: [scripts]

tasks:
  scripts:
    help: Run all scripts
    foreach:
      glob: "scripts/*.sh"
      as: script
    bash: bash "${script}"
    "#;

    fs::write(&otto_file, config).unwrap();

    // Run from the docs subdirectory
    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "scripts".to_string(),
    ];

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(&docs_dir).unwrap();

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse();

    std::env::set_current_dir(&original_dir).unwrap();

    let (tasks, _, _, _, _, _) = result
        .map(|o| o.into_run().unwrap().into_parts())
        .expect("Parse should succeed from docs subdirectory");

    // Should find all 3 shell scripts
    let foreach_subtasks: Vec<_> = tasks
        .iter()
        .filter(|t| t.name.starts_with("scripts:") && t.name != "scripts")
        .collect();

    assert_eq!(
        foreach_subtasks.len(),
        3,
        "Should have 3 foreach subtasks for build.sh, test.sh, deploy.sh, got: {:?}",
        foreach_subtasks.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
}

#[test]
#[serial]
fn test_foreach_items_list_works_from_subdirectory() {
    // This test verifies that items-based foreach (not glob) also works from subdirectories
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    let subdir = root.join("subdir");
    fs::create_dir(&subdir).unwrap();

    let otto_file = root.join("otto.yml");

    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  deploy:
    help: Deploy to environments
    foreach:
      items: [dev, staging, prod]
      as: env
    bash: echo "Deploying to ${env}"
    "#;

    fs::write(&otto_file, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "deploy".to_string(),
    ];

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(&subdir).unwrap();

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse();

    std::env::set_current_dir(&original_dir).unwrap();

    let (tasks, _, _, _, _, _) = result
        .map(|o| o.into_run().unwrap().into_parts())
        .expect("Parse should succeed from subdirectory");

    let foreach_subtasks: Vec<_> = tasks
        .iter()
        .filter(|t| t.name.starts_with("deploy:") && t.name != "deploy")
        .collect();

    assert_eq!(
        foreach_subtasks.len(),
        3,
        "Should have 3 foreach subtasks for dev, staging, prod, got: {:?}",
        foreach_subtasks.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    // Verify subtask names are correct
    let subtask_names: Vec<_> = foreach_subtasks.iter().map(|t| t.name.as_str()).collect();
    assert!(subtask_names.contains(&"deploy:dev"));
    assert!(subtask_names.contains(&"deploy:staging"));
    assert!(subtask_names.contains(&"deploy:prod"));
}
