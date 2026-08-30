mod common;

use common::otto_cmd;
use serial_test::serial;
use std::fs;
use tempfile::TempDir;

/// Test that all four built-in commands are registered and show up in help
#[test]
#[serial]
fn test_all_builtin_commands_registered() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let otto_home = temp_dir.path().join(".otto");
    let ottofile = temp_dir.path().join("otto.yml");

    // Create minimal ottofile
    fs::write(
        &ottofile,
        r#"
tasks:
  dummy:
    action: echo "test"
"#,
    )?;

    let output = otto_cmd(&otto_home)
        .current_dir(temp_dir.path())
        .arg("--help")
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // All built-in commands MUST appear in help (capitalized)
    assert!(
        stdout.contains("Graph") && stdout.contains("[built-in]"),
        "Graph command not found in help"
    );
    assert!(
        stdout.contains("Clean") && stdout.contains("[built-in]"),
        "Clean command not found in help"
    );
    assert!(
        stdout.contains("History") && stdout.contains("[built-in]"),
        "History command not found in help"
    );
    assert!(
        stdout.contains("Stats") && stdout.contains("[built-in]"),
        "Stats command not found in help"
    );
    assert!(
        stdout.contains("Convert") && stdout.contains("[built-in]"),
        "Convert command not found in help"
    );

    Ok(())
}

/// Test that graph command can be invoked
#[test]
#[serial]
fn test_graph_command_exists() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let otto_home = temp_dir.path().join(".otto");
    let ottofile = temp_dir.path().join("otto.yml");

    fs::write(
        &ottofile,
        r#"
tasks:
  test:
    action: echo "test"
"#,
    )?;

    let mut cmd = otto_cmd(&otto_home);
    let output = cmd.current_dir(temp_dir.path()).arg("Graph").arg("--help").output()?;

    assert!(output.status.success(), "Graph --help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Graph") || stdout.contains("Visualize"),
        "Graph help should mention Graph/visualize"
    );

    Ok(())
}

/// Test that Clean command can be invoked
#[test]
#[serial]
fn test_clean_command_exists() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let otto_home = temp_dir.path().join(".otto");

    let mut cmd = otto_cmd(&otto_home);
    let output = cmd.arg("Clean").arg("--help").output()?;

    assert!(output.status.success(), "Clean --help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Clean"), "Clean help should mention Clean");

    Ok(())
}

/// Test that History command can be invoked
#[test]
#[serial]
fn test_history_command_exists() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let otto_home = temp_dir.path().join(".otto");

    let mut cmd = otto_cmd(&otto_home);
    let output = cmd.arg("History").arg("--help").output()?;

    assert!(output.status.success(), "History --help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("History") || stdout.contains("execution history"),
        "History help should mention History"
    );

    Ok(())
}

/// Test that Stats command can be invoked
#[test]
#[serial]
fn test_stats_command_exists() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let otto_home = temp_dir.path().join(".otto");

    let mut cmd = otto_cmd(&otto_home);
    let output = cmd.arg("Stats").arg("--help").output()?;

    assert!(output.status.success(), "Stats --help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Stats") || stdout.contains("statistics"),
        "Stats help should mention Stats"
    );

    Ok(())
}

/// Test that all built-in commands are filtered out during normal execution
#[test]
#[serial]
fn test_builtin_commands_filtered_from_execution() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let otto_home = temp_dir.path().join(".otto");
    let ottofile = temp_dir.path().join("otto.yml");

    // Create ottofile with a task that depends on a built-in
    fs::write(
        &ottofile,
        r#"
tasks:
  real-task:
    action: echo "executing real task"
"#,
    )?;

    // If we try to run "graph real-task", it should handle graph specially
    // and then execute real-task normally
    let mut cmd = otto_cmd(&otto_home);
    let output = cmd.current_dir(temp_dir.path()).arg("real-task").output()?;

    // Should succeed - real-task executes
    assert!(output.status.success(), "real-task should execute successfully");

    Ok(())
}

/// Test count of built-in commands (regression test)
#[test]
#[serial]
fn test_builtin_command_count() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let otto_home = temp_dir.path().join(".otto");
    let ottofile = temp_dir.path().join("otto.yml");

    fs::write(
        &ottofile,
        r#"
tasks:
  dummy:
    action: echo "test"
"#,
    )?;

    let mut cmd = otto_cmd(&otto_home);
    let output = cmd.current_dir(temp_dir.path()).arg("--help").output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Count how many times "[built-in]" appears
    let builtin_count = stdout.matches("[built-in]").count();

    assert_eq!(
        builtin_count, 6,
        "Expected exactly 6 built-in commands, found {}. Commands: Clean, Convert, Graph, History, Stats, Upgrade",
        builtin_count
    );

    Ok(())
}

/// Test that built-in commands have proper help text
#[test]
#[serial]
fn test_builtin_commands_have_help() -> Result<(), Box<dyn std::error::Error>> {
    let commands = vec![
        ("Graph", "Visualize", "--format"),
        ("Clean", "Clean", "--keep"),
        ("History", "History", "--limit"),
        ("Stats", "statistics", "--limit"),
        ("Convert", "Convert", "--output"),
    ];

    for (cmd_name, expected_word, expected_flag) in commands {
        let temp_dir = TempDir::new()?;
        let otto_home = temp_dir.path().join(".otto");

        let mut cmd = otto_cmd(&otto_home);
        let output = cmd.arg(cmd_name).arg("--help").output()?;

        assert!(output.status.success(), "{} --help should succeed", cmd_name);

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.to_lowercase().contains(&expected_word.to_lowercase()),
            "{} help should mention '{}'",
            cmd_name,
            expected_word
        );
        assert!(
            stdout.contains(expected_flag),
            "{} help should mention flag '{}'",
            cmd_name,
            expected_flag
        );
    }

    Ok(())
}
