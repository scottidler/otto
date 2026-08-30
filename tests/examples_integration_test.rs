/// Integration tests for example otto.yml files
/// These tests ensure examples stay working as the codebase evolves
mod common;

use assert_cmd::Command;
use tempfile::TempDir;

/// Helper to get the otto binary path, isolated to a throwaway `OTTO_HOME` so
/// running the examples doesn't write into the developer's real
/// `~/.otto/otto.db`. The `TempDir` lives only for the caller's synchronous
/// `.output()`/`.assert()` call, which is fine since nothing outlives it.
fn otto_cmd() -> (TempDir, Command) {
    let home = TempDir::new().expect("failed to create scratch OTTO_HOME");
    let cmd = common::otto_cmd(home.path());
    (home, cmd)
}

/// Helper to run an example and verify it succeeds
fn run_example(example_dir: &str, task: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (_home, mut cmd) = otto_cmd();
    cmd.current_dir(format!("examples/{}", example_dir));
    cmd.arg(task);

    let output = cmd.output()?;

    if !output.status.success() {
        eprintln!("=== STDOUT ===");
        eprintln!("{}", String::from_utf8_lossy(&output.stdout));
        eprintln!("=== STDERR ===");
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return Err(format!(
            "Example {} failed with exit code: {:?}",
            example_dir,
            output.status.code()
        )
        .into());
    }

    Ok(())
}

/// Helper to just validate an example parses correctly
fn validate_example_parses(example_dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (_home, mut cmd) = otto_cmd();
    cmd.current_dir(format!("examples/{}", example_dir));
    cmd.arg("--help");

    let output = cmd.output()?;

    if !output.status.success() {
        eprintln!("=== STDERR ===");
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return Err(format!("Example {} failed to parse", example_dir).into());
    }

    // Should show task list in help
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Commands:") || stdout.contains("Usage:"),
        "Help output should show commands for {}",
        example_dir
    );

    Ok(())
}

// ============================================================================
// Simple Examples - Just verify they parse
// ============================================================================

#[test]
fn test_hello_world_parses() {
    validate_example_parses("hello-world").expect("hello-world should parse");
}

#[test]
fn test_basic_dependencies_parses() {
    validate_example_parses("basic-dependencies").expect("basic-dependencies should parse");
}

#[test]
fn test_dependency_ordering_parses() {
    validate_example_parses("dependency-ordering").expect("dependency-ordering should parse");
}

#[test]
fn test_diamond_dependencies_parses() {
    validate_example_parses("diamond-dependencies").expect("diamond-dependencies should parse");
}

#[test]
fn test_complex_workflow_parses() {
    validate_example_parses("complex-workflow").expect("complex-workflow should parse");
}

#[test]
fn test_parallel_tasks_parses() {
    validate_example_parses("parallel-tasks").expect("parallel-tasks should parse");
}

#[test]
fn test_file_dependencies_parses() {
    validate_example_parses("file-dependencies").expect("file-dependencies should parse");
}

#[test]
fn test_build_pipeline_parses() {
    validate_example_parses("build-pipeline").expect("build-pipeline should parse");
}

#[test]
fn test_environment_variables_parses() {
    validate_example_parses("environment-variables").expect("environment-variables should parse");
}

#[test]
fn test_build_test_deploy_parses() {
    validate_example_parses("build-test-deploy").expect("build-test-deploy should parse");
}

// ============================================================================
// Examples That Should Execute Successfully
// ============================================================================

#[test]
fn test_hello_world_punch_executes() {
    run_example("hello-world", "punch").expect("hello-world punch should execute successfully");
}

#[test]
fn test_data_flow_bash() {
    // This tests the data passing mechanism (bash only)
    run_example("data-flow-bash", "consume").expect("data-flow-bash consume should execute successfully");
}

#[test]
fn test_data_passing_demo_validation() {
    // This tests data passing with bash and python
    run_example("data-passing-demo", "report").expect("data-passing-demo report should execute successfully");
}

// ============================================================================
// Makefile Examples - Verify they parse (moved to makefiles/)
// ============================================================================

#[test]
fn test_makefile_python_poetry_service_parses() {
    let (_home, mut cmd) = otto_cmd();
    cmd.current_dir("makefiles/python-poetry-service");
    cmd.arg("--help");
    let output = cmd.output().expect("Failed to run otto");
    assert!(output.status.success(), "python-poetry-service should parse");
}

#[test]
fn test_makefile_go_build_project_parses() {
    let (_home, mut cmd) = otto_cmd();
    cmd.current_dir("makefiles/go-build-project");
    cmd.arg("--help");
    let output = cmd.output().expect("Failed to run otto");
    assert!(output.status.success(), "go-build-project should parse");
}

#[test]
fn test_makefile_python_pre_commit_parses() {
    let (_home, mut cmd) = otto_cmd();
    cmd.current_dir("makefiles/python-pre-commit");
    cmd.arg("--help");
    let output = cmd.output().expect("Failed to run otto");
    assert!(output.status.success(), "python-pre-commit should parse");
}

#[test]
fn test_makefile_docker_compose_service_parses() {
    let (_home, mut cmd) = otto_cmd();
    cmd.current_dir("makefiles/docker-compose-service");
    cmd.arg("--help");
    let output = cmd.output().expect("Failed to run otto");
    assert!(output.status.success(), "docker-compose-service should parse");
}

#[test]
fn test_makefile_example_parses() {
    let (_home, mut cmd) = otto_cmd();
    cmd.current_dir("makefiles/makefile-example");
    cmd.arg("--help");
    let output = cmd.output().expect("Failed to run otto");
    assert!(output.status.success(), "makefile-example should parse");
}

// ============================================================================
// Special Examples - Just verify they parse (can't run without interaction)
// ============================================================================

#[test]
fn test_interactive_demo_parses() {
    validate_example_parses("interactive-demo").expect("interactive-demo should parse");
}

#[test]
fn test_tui_demo_parses() {
    validate_example_parses("tui-demo").expect("tui-demo should parse");
}

// ============================================================================
// Negative Test - Verify we catch broken examples
// ============================================================================

#[test]
#[should_panic(expected = "should parse")]
fn test_validates_broken_examples() {
    // This would fail if we had a broken example (good!)
    // Create a temporary broken example
    std::fs::create_dir_all("examples/_test_broken").ok();
    std::fs::write("examples/_test_broken/otto.yml", "invalid: yaml: content: [[[").ok();

    validate_example_parses("_test_broken").expect("should parse");

    // Cleanup
    std::fs::remove_dir_all("examples/_test_broken").ok();
}
