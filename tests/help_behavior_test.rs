mod common;

use predicates::prelude::*;
use std::fs;
use std::io::Write;
use tempfile::tempdir;

#[test]
fn test_help_with_ottofile_shows_tasks_and_no_error() {
    let temp = tempdir().unwrap();
    let ottofile_path = temp.path().join("otto.yml");
    let mut file = fs::File::create(&ottofile_path).unwrap();
    writeln!(
        file,
        r#"
otto:
  api: 1
tasks:
  build:
    help: Build the project
    action: echo building
  test:
    help: Run tests
    action: echo testing
  deploy:
    help: Deploy the application
    action: echo deploying
"#
    )
    .unwrap();

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).arg("--help");

    cmd.assert()
        .success()
        .code(0)
        // Must contain the standard help structure
        .stdout(predicate::str::contains("A task runner"))
        .stdout(predicate::str::contains("Usage: otto [OPTIONS] [COMMAND]"))
        .stdout(predicate::str::contains("Commands:"))
        .stdout(predicate::str::contains("Options:"))
        // Must contain the tasks as subcommands
        .stdout(predicate::str::contains("build"))
        .stdout(predicate::str::contains("test"))
        .stdout(predicate::str::contains("deploy"))
        .stdout(predicate::str::contains("graph"))
        // Must contain standard options
        .stdout(predicate::str::contains("-j, --jobs"))
        .stdout(predicate::str::contains("-t, --tui"))
        // Must NOT contain error message when ottofile exists
        .stdout(predicate::str::contains("ERROR: No ottofile found").not())
        .stdout(predicate::str::contains("Otto looks for one of the following files").not());
}

#[test]
fn test_help_without_ottofile_shows_normal_help_plus_error_message() {
    let temp = tempdir().unwrap();
    // No ottofile created - directory is empty

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).arg("--help");

    cmd.assert()
        .failure()
        .code(2)
        // Must STILL contain the standard help structure
        .stdout(predicate::str::contains("A task runner"))
        .stdout(predicate::str::contains("Usage: otto [OPTIONS] [COMMAND]"))
        .stdout(predicate::str::contains("Options:"))
        // Must contain standard options
        .stdout(predicate::str::contains("-j, --jobs"))
        .stdout(predicate::str::contains("-t, --tui"))
        // Must NOT contain Commands section since no tasks available
        .stdout(predicate::str::contains("Commands:").not())
        // Must contain error message as after_help
        .stdout(predicate::str::contains(
            "ERROR: No ottofile found in this directory or any parent directory!",
        ))
        .stdout(predicate::str::contains("Otto looks for one of the following files"))
        .stdout(predicate::str::contains("otto.yml"))
        .stdout(predicate::str::contains(".otto.yml"))
        .stdout(predicate::str::contains("otto.yaml"))
        .stdout(predicate::str::contains(".otto.yaml"))
        .stdout(predicate::str::contains("Ottofile"))
        .stdout(predicate::str::contains("OTTOFILE"));
}

#[test]
fn test_help_with_short_flag_behaves_same_as_long_flag() {
    let temp = tempdir().unwrap();
    // No ottofile created - directory is empty

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).arg("-h");

    cmd.assert()
        .failure()
        .code(2)
        // Must contain the same content as --help
        .stdout(predicate::str::contains("A task runner"))
        .stdout(predicate::str::contains("Usage: otto [OPTIONS] [COMMAND]"))
        .stdout(predicate::str::contains(
            "ERROR: No ottofile found in this directory or any parent directory!",
        ));
}

#[test]
fn test_help_with_complex_ottofile_shows_all_tasks() {
    let temp = tempdir().unwrap();
    let ottofile_path = temp.path().join("otto.yml");
    let mut file = fs::File::create(&ottofile_path).unwrap();
    writeln!(
        file,
        r#"
otto:
  api: 1
tasks:
  all:
    help: Run all development tasks
    action: echo all
  clean:
    help: Clean build artifacts and caches
    action: echo clean
  dev:
    help: Install development dependencies
    action: echo dev
  integration-test:
    help: Run integration tests
    action: echo integration-test
  keygen:
    help: Generate RSA key pairs
    action: echo keygen
  lint:
    help: Run linting with pre-commit hooks
    action: echo lint
  type-check:
    help: Run type checking with mypy
    action: echo type-check
  unit-test:
    help: Run unit tests with pytest and coverage
    action: echo unit-test
"#
    )
    .unwrap();

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).arg("--help");

    cmd.assert()
        .success()
        .code(0)
        // Must contain all the tasks
        .stdout(predicate::str::contains("all"))
        .stdout(predicate::str::contains("clean"))
        .stdout(predicate::str::contains("dev"))
        .stdout(predicate::str::contains("integration-test"))
        .stdout(predicate::str::contains("keygen"))
        .stdout(predicate::str::contains("lint"))
        .stdout(predicate::str::contains("type-check"))
        .stdout(predicate::str::contains("unit-test"))
        // Must contain built-in graph task
        .stdout(predicate::str::contains("graph"))
        .stdout(predicate::str::contains(
            "[built-in] Visualize the task dependency graph",
        ))
        // Must NOT contain error message
        .stdout(predicate::str::contains("ERROR: No ottofile found").not());
}

#[test]
fn test_help_error_message_order_is_after_main_help() {
    let temp = tempdir().unwrap();

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).arg("--help");

    let output = cmd.assert().failure().code(2);

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);

    // Find positions of key elements
    let usage_pos = stdout
        .find("Usage: otto [OPTIONS] [COMMAND]")
        .expect("Usage should be present");
    let options_pos = stdout.find("Options:").expect("Options should be present");
    let error_pos = stdout
        .find("ERROR: No ottofile found")
        .expect("Error should be present");

    // Error message should come AFTER the main help content
    assert!(error_pos > usage_pos, "Error message should come after Usage");
    assert!(error_pos > options_pos, "Error message should come after Options");
}

#[test]
fn test_help_with_ottofile_in_parent_directory() {
    let temp = tempdir().unwrap();
    let ottofile_path = temp.path().join("otto.yml");
    let mut file = fs::File::create(&ottofile_path).unwrap();
    writeln!(
        file,
        r#"
otto:
  api: 1
tasks:
  parent-task:
    help: Task from parent directory
    action: echo parent
"#
    )
    .unwrap();

    // Create subdirectory
    let subdir = temp.path().join("subdir");
    fs::create_dir(&subdir).unwrap();

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&subdir).arg("--help");

    cmd.assert()
        .success()
        .code(0)
        // Should find the ottofile in parent directory and show tasks
        .stdout(predicate::str::contains("parent-task"))
        .stdout(predicate::str::contains("Commands:"))
        // Should NOT show error message
        .stdout(predicate::str::contains("ERROR: No ottofile found").not());
}

#[test]
fn test_help_behavior_is_consistent_across_different_ottofile_names() {
    let ottofile_names = vec![
        "otto.yml",
        ".otto.yml",
        "otto.yaml",
        ".otto.yaml",
        "Ottofile",
        "OTTOFILE",
    ];

    for ottofile_name in ottofile_names {
        let temp = tempdir().unwrap();
        let ottofile_path = temp.path().join(ottofile_name);
        let mut file = fs::File::create(&ottofile_path).unwrap();
        writeln!(
            file,
            r#"
otto:
  api: 1
tasks:
  test-task:
    help: Test task
    action: echo test
"#
        )
        .unwrap();

        let mut cmd = common::otto_cmd(temp.path());
        cmd.current_dir(&temp).arg("--help");

        cmd.assert()
            .success()
            .code(0)
            // Should show tasks for any valid ottofile name
            .stdout(predicate::str::contains("test-task"))
            .stdout(predicate::str::contains("Commands:"))
            // Should NOT show error message
            .stdout(predicate::str::contains("ERROR: No ottofile found").not());
    }
}

#[test]
fn test_task_help_with_long_flag() {
    let temp = tempdir().unwrap();
    let ottofile_path = temp.path().join("otto.yml");
    let mut file = fs::File::create(&ottofile_path).unwrap();
    writeln!(
        file,
        r#"
otto:
  api: 1
tasks:
  build:
    help: Build the project
    bash: echo building
  test:
    help: Run tests
    bash: echo testing
"#
    )
    .unwrap();

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).args(["build", "--help"]);

    cmd.assert()
        .success()
        .code(0)
        // Should show task-specific help
        .stdout(predicate::str::contains("Build the project"))
        .stdout(predicate::str::contains("Usage: otto build"))
        // Should NOT run the task
        .stdout(predicate::str::contains("building").not());
}

#[test]
fn test_task_help_with_short_flag() {
    let temp = tempdir().unwrap();
    let ottofile_path = temp.path().join("otto.yml");
    let mut file = fs::File::create(&ottofile_path).unwrap();
    writeln!(
        file,
        r#"
otto:
  api: 1
tasks:
  build:
    help: Build the project
    bash: echo building
"#
    )
    .unwrap();

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).args(["build", "-h"]);

    cmd.assert()
        .success()
        .code(0)
        // Should show task-specific help
        .stdout(predicate::str::contains("Build the project"))
        .stdout(predicate::str::contains("Usage: otto build"))
        // Should NOT run the task
        .stdout(predicate::str::contains("building").not());
}

#[test]
fn test_foreach_task_help_with_long_flag() {
    let temp = tempdir().unwrap();
    let ottofile_path = temp.path().join("otto.yml");
    let mut file = fs::File::create(&ottofile_path).unwrap();
    writeln!(
        file,
        r#"
otto:
  api: 1
tasks:
  examples:
    help: Run all examples
    foreach:
      items: [one, two, three]
      as: item
    bash: |
      echo "Running example: $item"
"#
    )
    .unwrap();

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).args(["examples", "--help"]);

    cmd.assert()
        .success()
        .code(0)
        // Should show task-specific help
        .stdout(predicate::str::contains("Run all examples"))
        .stdout(predicate::str::contains("Usage: otto examples"))
        // Should NOT run the task (no example output)
        .stdout(predicate::str::contains("Running example").not());
}

#[test]
fn test_foreach_task_help_with_short_flag() {
    let temp = tempdir().unwrap();
    let ottofile_path = temp.path().join("otto.yml");
    let mut file = fs::File::create(&ottofile_path).unwrap();
    writeln!(
        file,
        r#"
otto:
  api: 1
tasks:
  examples:
    help: Run all examples
    foreach:
      items: [one, two, three]
      as: item
    bash: |
      echo "Running example: $item"
"#
    )
    .unwrap();

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).args(["examples", "-h"]);

    cmd.assert()
        .success()
        .code(0)
        // Should show task-specific help
        .stdout(predicate::str::contains("Run all examples"))
        .stdout(predicate::str::contains("Usage: otto examples"))
        // Should NOT run the task
        .stdout(predicate::str::contains("Running example").not());
}

#[test]
fn test_task_with_params_help_shows_params() {
    let temp = tempdir().unwrap();
    let ottofile_path = temp.path().join("otto.yml");
    let mut file = fs::File::create(&ottofile_path).unwrap();
    writeln!(
        file,
        r#"
otto:
  api: 1
tasks:
  deploy:
    help: Deploy the application
    params:
      --env:
        help: Target environment
        default: dev
      --verbose:
        help: Enable verbose output
    bash: echo "Deploying to $env"
"#
    )
    .unwrap();

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).args(["deploy", "--help"]);

    cmd.assert()
        .success()
        .code(0)
        // Should show task help
        .stdout(predicate::str::contains("Deploy the application"))
        // Should show params
        .stdout(predicate::str::contains("--env"))
        .stdout(predicate::str::contains("Target environment"))
        .stdout(predicate::str::contains("--verbose"))
        .stdout(predicate::str::contains("Enable verbose output"))
        // Should NOT run the task
        .stdout(predicate::str::contains("Deploying to").not());
}

#[test]
fn test_foreach_task_help_shows_builtin_serial_flag() {
    let temp = tempdir().unwrap();
    let ottofile_path = temp.path().join("otto.yml");
    let mut file = fs::File::create(&ottofile_path).unwrap();
    writeln!(
        file,
        r#"
otto:
  api: 1
tasks:
  examples:
    help: Run all examples
    foreach:
      items: [one, two, three]
      as: item
    bash: echo "Running $item"
"#
    )
    .unwrap();

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).args(["examples", "--help"]);

    cmd.assert()
        .success()
        .code(0)
        // Should show builtin --Serial flag
        .stdout(predicate::str::contains("--Serial"))
        .stdout(predicate::str::contains("[builtin]"));
}

#[test]
fn test_non_foreach_task_does_not_have_serial_flag() {
    let temp = tempdir().unwrap();
    let ottofile_path = temp.path().join("otto.yml");
    let mut file = fs::File::create(&ottofile_path).unwrap();
    writeln!(
        file,
        r#"
otto:
  api: 1
tasks:
  build:
    help: Build the project
    bash: echo building
"#
    )
    .unwrap();

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).args(["build", "--help"]);

    cmd.assert()
        .success()
        .code(0)
        // Should NOT have --Serial flag (not a foreach task)
        .stdout(predicate::str::contains("--Serial").not());
}

#[test]
fn test_reserved_builtin_param_errors_at_parse_time() {
    let temp = tempdir().unwrap();
    let ottofile_path = temp.path().join("otto.yml");
    let mut file = fs::File::create(&ottofile_path).unwrap();
    writeln!(
        file,
        r#"
otto:
  api: 1
tasks:
  test:
    help: Test task
    params:
      --Serial:
        help: My serial flag
    bash: echo test
"#
    )
    .unwrap();

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).args(["test"]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("reserved builtin param"));
}

/// `otto --help` from a subdirectory resolves foreach globs against the
/// ottofile's directory, not the cwd.
///
/// The `--help` path builds a throwaway parser to render the task list, and it
/// built it with `ottofile: None`, which makes `base_dir()` the cwd. So the
/// item count in a foreach task's help was whatever the glob happened to match
/// in the directory the user was standing in: a real count at the project root,
/// `[foreach]` one level down.
/// (`docs/design/2026-09-02-second-code-review-remediation.md`, Phase 5)
#[test]
fn help_from_a_subdirectory_counts_foreach_items_from_the_ottofile_dir() {
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("otto.yml"),
        r#"
otto:
  api: 1
tasks:
  each:
    help: Per item
    foreach:
      glob: "items/*.txt"
      as: item
    action: echo "$item"
"#,
    )
    .unwrap();
    fs::create_dir(temp.path().join("items")).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        fs::write(temp.path().join("items").join(name), "x").unwrap();
    }
    let subdir = temp.path().join("deep");
    fs::create_dir(&subdir).unwrap();

    common::otto_cmd(&temp.path().join(".otto"))
        .current_dir(&subdir)
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("[3 items]"));
}
