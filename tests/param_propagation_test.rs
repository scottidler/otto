use otto::cli::parser::{Parser, Task};
use serial_test::serial;
use std::fs;
use tempfile::TempDir;

/// Helper: find a task by name in the parsed task list
fn find_task<'a>(tasks: &'a [Task], name: &str) -> &'a Task {
    tasks.iter().find(|t| t.name == name).unwrap_or_else(|| {
        panic!(
            "Task '{}' not found in: {:?}",
            name,
            tasks.iter().map(|t| &t.name).collect::<Vec<_>>()
        )
    })
}

/// Helper: parse an otto config with given CLI args
fn parse_config(config: &str, cli_args: Vec<&str>) -> Result<Vec<Task>, eyre::Report> {
    let temp_dir = TempDir::new().unwrap();
    let otto_file = temp_dir.path().join("otto.yml");
    fs::write(&otto_file, config).unwrap();

    let mut args: Vec<String> = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
    ];
    args.extend(cli_args.into_iter().map(String::from));

    let mut parser = Parser::new(args)?;
    let (tasks, _, _, _, _, _) = parser.parse()?.into_run()?.into_parts();
    Ok(tasks)
}

// =============================================================================
// Basic propagation
// =============================================================================

#[test]
#[serial]
fn test_single_level_propagation() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  build:
    params:
      --account:
        default: home
        help: Account to use
    bash: echo "build ${account}"

  deploy:
    params:
      --account:
        default: home
        help: Account to use
    before: [build]
    bash: echo "deploy ${account}"
"#;

    let tasks = parse_config(config, vec!["deploy", "--account", "work"]).unwrap();

    let deploy = find_task(&tasks, "deploy");
    assert_eq!(deploy.envs.get("account").unwrap(), "work");

    let build = find_task(&tasks, "build");
    assert_eq!(
        build.envs.get("account").unwrap(),
        "work",
        "build should inherit account=work from deploy"
    );
}

#[test]
#[serial]
fn test_transitive_propagation() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  build:
    params:
      --account:
        default: home
        help: Account to use
    bash: echo "build ${account}"

  middle:
    params:
      --account:
        default: home
        help: Account to use
    before: [build]
    bash: echo "middle ${account}"

  deploy:
    params:
      --account:
        default: home
        help: Account to use
    before: [middle]
    bash: echo "deploy ${account}"
"#;

    let tasks = parse_config(config, vec!["deploy", "--account", "work"]).unwrap();

    assert_eq!(find_task(&tasks, "deploy").envs.get("account").unwrap(), "work");
    assert_eq!(
        find_task(&tasks, "middle").envs.get("account").unwrap(),
        "work",
        "middle should inherit account=work from deploy"
    );
    assert_eq!(
        find_task(&tasks, "build").envs.get("account").unwrap(),
        "work",
        "build should inherit account=work transitively through middle"
    );
}

#[test]
#[serial]
fn test_chain_breaks_when_intermediate_lacks_param() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  build:
    params:
      --account:
        default: home
        help: Account to use
    bash: echo "build ${account}"

  middle:
    before: [build]
    bash: echo "middle has no account param"

  deploy:
    params:
      --account:
        default: home
        help: Account to use
    before: [middle]
    bash: echo "deploy ${account}"
"#;

    let tasks = parse_config(config, vec!["deploy", "--account", "work"]).unwrap();

    assert_eq!(find_task(&tasks, "deploy").envs.get("account").unwrap(), "work");
    // middle doesn't declare --account so it can't propagate
    assert!(
        !find_task(&tasks, "middle").envs.contains_key("account"),
        "middle should not have account since it doesn't declare the param"
    );
    // build gets its default because middle broke the chain
    assert_eq!(
        find_task(&tasks, "build").envs.get("account").unwrap(),
        "home",
        "build should get default since middle doesn't propagate account"
    );
}

// =============================================================================
// CLI override
// =============================================================================

#[test]
#[serial]
fn test_cli_override_prevents_inheritance() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy, build]

tasks:
  build:
    params:
      --account:
        default: home
        help: Account to use
    bash: echo "build ${account}"

  deploy:
    params:
      --account:
        default: home
        help: Account to use
    before: [build]
    bash: echo "deploy ${account}"
"#;

    let tasks = parse_config(
        config,
        vec!["deploy", "--account", "work", "build", "--account", "staging"],
    )
    .unwrap();

    assert_eq!(find_task(&tasks, "deploy").envs.get("account").unwrap(), "work");
    assert_eq!(
        find_task(&tasks, "build").envs.get("account").unwrap(),
        "staging",
        "build's explicit CLI value should win over propagation from deploy"
    );
}

// =============================================================================
// Diamond dependencies
// =============================================================================

#[test]
#[serial]
fn test_diamond_conflict_produces_error() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy-staging, deploy-prod]

tasks:
  build:
    params:
      --account:
        default: home
        help: Account to use
    bash: echo "build ${account}"

  deploy-staging:
    params:
      --account:
        default: staging
        help: Account to use
    before: [build]
    bash: echo "deploy staging ${account}"

  deploy-prod:
    params:
      --account:
        default: prod
        help: Account to use
    before: [build]
    bash: echo "deploy prod ${account}"
"#;

    // Both parents provide different values for account via CLI
    let result = parse_config(
        config,
        vec![
            "deploy-staging",
            "--account",
            "staging",
            "deploy-prod",
            "--account",
            "prod",
        ],
    );

    assert!(result.is_err(), "Should error on diamond conflict");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Conflicting param propagation"),
        "Error should mention conflict: {}",
        err
    );
    assert!(err.contains("account"), "Error should mention the param name: {}", err);
}

#[test]
#[serial]
fn test_diamond_agreement_succeeds() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy-a, deploy-b]

tasks:
  build:
    params:
      --account:
        default: home
        help: Account to use
    bash: echo "build ${account}"

  deploy-a:
    params:
      --account:
        default: home
        help: Account to use
    before: [build]
    bash: echo "deploy-a ${account}"

  deploy-b:
    params:
      --account:
        default: home
        help: Account to use
    before: [build]
    bash: echo "deploy-b ${account}"
"#;

    // Both parents propagate the same value — should succeed
    let tasks = parse_config(
        config,
        vec!["deploy-a", "--account", "work", "deploy-b", "--account", "work"],
    )
    .unwrap();

    assert_eq!(
        find_task(&tasks, "build").envs.get("account").unwrap(),
        "work",
        "build should inherit agreed-upon value from both parents"
    );
}

// =============================================================================
// Choices validation
// =============================================================================

#[test]
#[serial]
fn test_choices_validation_on_propagated_value() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  build:
    params:
      --env:
        default: dev
        choices: [dev, staging]
        help: Target environment
    bash: echo "build ${env}"

  deploy:
    params:
      --env:
        default: dev
        choices: [dev, staging, prod]
        help: Target environment
    before: [build]
    bash: echo "deploy ${env}"
"#;

    // deploy allows prod but build only allows dev/staging
    let result = parse_config(config, vec!["deploy", "--env", "prod"]);

    assert!(result.is_err(), "Should error when propagated value violates choices");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not in allowed choices"),
        "Error should mention choices: {}",
        err
    );
    assert!(err.contains("prod"), "Error should mention the value: {}", err);
}

#[test]
#[serial]
fn test_choices_validation_passes_for_valid_propagated_value() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  build:
    params:
      --env:
        default: dev
        choices: [dev, staging, prod]
        help: Target environment
    bash: echo "build ${env}"

  deploy:
    params:
      --env:
        default: dev
        choices: [dev, staging, prod]
        help: Target environment
    before: [build]
    bash: echo "deploy ${env}"
"#;

    let tasks = parse_config(config, vec!["deploy", "--env", "staging"]).unwrap();

    assert_eq!(
        find_task(&tasks, "build").envs.get("env").unwrap(),
        "staging",
        "Valid propagated value should pass choices validation"
    );
}

// =============================================================================
// No propagation cases
// =============================================================================

#[test]
#[serial]
fn test_dep_invoked_directly_gets_default() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy, build]

tasks:
  build:
    params:
      --account:
        default: home
        help: Account to use
    bash: echo "build ${account}"

  deploy:
    params:
      --account:
        default: home
        help: Account to use
    before: [build]
    bash: echo "deploy ${account}"
"#;

    // Only invoke build directly — deploy is not in the graph
    let tasks = parse_config(config, vec!["build"]).unwrap();

    assert_eq!(tasks.len(), 1);
    assert_eq!(
        find_task(&tasks, "build").envs.get("account").unwrap(),
        "home",
        "build should use its default when invoked directly"
    );
}

#[test]
#[serial]
fn test_no_propagation_without_cli_value() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  build:
    params:
      --account:
        default: default-build
        help: Account to use
    bash: echo "build ${account}"

  deploy:
    params:
      --account:
        default: default-deploy
        help: Account to use
    before: [build]
    bash: echo "deploy ${account}"
"#;

    // No CLI value — deploy gets its default, which then propagates to build
    let tasks = parse_config(config, vec!["deploy"]).unwrap();

    assert_eq!(
        find_task(&tasks, "deploy").envs.get("account").unwrap(),
        "default-deploy"
    );
    // Propagation happens in Phase 2 before defaults in Phase 3.
    // deploy has no CLI value, so Phase 1 sets nothing. Phase 2 has nothing to propagate.
    // Phase 3 applies defaults to both.
    assert_eq!(
        find_task(&tasks, "build").envs.get("account").unwrap(),
        "default-build",
        "Without CLI values, each task should get its own default"
    );
}

// =============================================================================
// Multiple params / partial propagation
// =============================================================================

#[test]
#[serial]
fn test_partial_propagation_multiple_params() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  build:
    params:
      --account:
        default: home
        help: Account to use
    bash: echo "build ${account}"

  deploy:
    params:
      --account:
        default: home
        help: Account to use
      --region:
        default: us-east-1
        help: Region to deploy to
    before: [build]
    bash: echo "deploy ${account} ${region}"
"#;

    let tasks = parse_config(config, vec!["deploy", "--account", "work", "--region", "eu-west-1"]).unwrap();

    assert_eq!(find_task(&tasks, "deploy").envs.get("account").unwrap(), "work");
    assert_eq!(find_task(&tasks, "deploy").envs.get("region").unwrap(), "eu-west-1");

    // build declares --account but not --region
    assert_eq!(
        find_task(&tasks, "build").envs.get("account").unwrap(),
        "work",
        "account should propagate since build declares it"
    );
    assert!(
        !find_task(&tasks, "build").envs.contains_key("region"),
        "region should NOT propagate since build doesn't declare it"
    );
}

// =============================================================================
// Foreach virtual parent handling
// =============================================================================

#[test]
#[serial]
fn test_propagation_to_foreach_subtasks() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  build:
    params:
      --account:
        default: home
        help: Account to use
    foreach:
      items: [frontend, backend]
      as: component
    bash: echo "build ${component} for ${account}"

  deploy:
    params:
      --account:
        default: home
        help: Account to use
    before: [build]
    bash: echo "deploy ${account}"
"#;

    let tasks = parse_config(config, vec!["deploy", "--account", "work"]).unwrap();

    assert_eq!(find_task(&tasks, "deploy").envs.get("account").unwrap(), "work");

    // Foreach subtasks should inherit the propagated value
    let frontend = find_task(&tasks, "build:frontend");
    assert_eq!(
        frontend.envs.get("account").unwrap(),
        "work",
        "foreach subtask build:frontend should inherit account=work"
    );

    let backend = find_task(&tasks, "build:backend");
    assert_eq!(
        backend.envs.get("account").unwrap(),
        "work",
        "foreach subtask build:backend should inherit account=work"
    );
}

/// The flag given to the foreach task *itself*, which is the shape a user
/// reaches for first and the one nothing covered. `as_virtual_parent()` empties
/// the parent's `params`, and the CLI-binding loop read the expanded spec, so
/// clap accepted `--account work` and the value was dropped on the floor at
/// exit 0. `test_propagation_to_foreach_subtasks` above passes without this
/// because it only ever gives the flag to a dependent parent.
#[test]
#[serial]
fn test_flag_given_directly_to_a_foreach_task_reaches_its_subtasks() {
    let config = r#"
otto:
  api: 1
  tasks: [build]

tasks:
  build:
    params:
      --account:
        default: home
        help: Account to use
    foreach:
      items: [frontend, backend]
      as: component
    bash: echo "build ${component} for ${account}"
"#;

    let tasks = parse_config(config, vec!["build", "--account", "work"]).unwrap();

    for subtask in ["build:frontend", "build:backend"] {
        assert_eq!(
            find_task(&tasks, subtask).envs.get("account").map(String::as_str),
            Some("work"),
            "{subtask} should see the value given to the foreach task itself, not the default"
        );
    }
}

/// Same shape with `--flag=value`, because the two spellings take different
/// paths through clap and only one of them was ever exercised.
#[test]
#[serial]
fn test_equals_form_given_directly_to_a_foreach_task_reaches_its_subtasks() {
    let config = r#"
otto:
  api: 1
  tasks: [build]

tasks:
  build:
    params:
      --account:
        default: home
        help: Account to use
    foreach:
      items: [frontend, backend]
      as: component
    bash: echo "build ${component} for ${account}"
"#;

    let tasks = parse_config(config, vec!["build", "--account=work"]).unwrap();

    for subtask in ["build:frontend", "build:backend"] {
        assert_eq!(
            find_task(&tasks, subtask).envs.get("account").map(String::as_str),
            Some("work"),
            "{subtask} should see the value from the --flag=value spelling"
        );
    }
}

/// Nothing given: the default still applies. Guards the fix against the
/// opposite failure, a parent that binds a clap default as if the user typed it
/// and thereby outranks a real propagated value.
#[test]
#[serial]
fn test_foreach_task_with_no_flag_still_takes_the_default() {
    let config = r#"
otto:
  api: 1
  tasks: [build]

tasks:
  build:
    params:
      --account:
        default: home
        help: Account to use
    foreach:
      items: [frontend, backend]
      as: component
    bash: echo "build ${component} for ${account}"
"#;

    let tasks = parse_config(config, vec!["build"]).unwrap();

    for subtask in ["build:frontend", "build:backend"] {
        assert_eq!(
            find_task(&tasks, subtask).envs.get("account").map(String::as_str),
            Some("home"),
            "{subtask} should fall back to the declared default"
        );
    }
}

// =============================================================================
// Flag (boolean) propagation
// =============================================================================

#[test]
#[serial]
fn test_boolean_flag_propagation() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  build:
    params:
      -v|--verbose:
        default: false
        help: Verbose output
    bash: echo "build verbose=${verbose}"

  deploy:
    params:
      -v|--verbose:
        default: false
        help: Verbose output
    before: [build]
    bash: echo "deploy verbose=${verbose}"
"#;

    let tasks = parse_config(config, vec!["deploy", "--verbose"]).unwrap();

    assert_eq!(find_task(&tasks, "deploy").envs.get("verbose").unwrap(), "true");
    assert_eq!(
        find_task(&tasks, "build").envs.get("verbose").unwrap(),
        "true",
        "boolean flag should propagate from deploy to build"
    );
}

#[test]
#[serial]
fn test_boolean_flag_not_set_uses_default() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  build:
    params:
      -v|--verbose:
        default: false
        help: Verbose output
    bash: echo "build verbose=${verbose}"

  deploy:
    params:
      -v|--verbose:
        default: false
        help: Verbose output
    before: [build]
    bash: echo "deploy verbose=${verbose}"
"#;

    // No --verbose flag, so both should get default=false
    let tasks = parse_config(config, vec!["deploy"]).unwrap();

    assert_eq!(find_task(&tasks, "deploy").envs.get("verbose").unwrap(), "false");
    assert_eq!(find_task(&tasks, "build").envs.get("verbose").unwrap(), "false");
}

// =============================================================================
// After relationship (normalized to before)
// =============================================================================

#[test]
#[serial]
fn test_propagation_through_after_relationship() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  build:
    params:
      --account:
        default: home
        help: Account to use
    after: [deploy]
    bash: echo "build ${account}"

  deploy:
    params:
      --account:
        default: home
        help: Account to use
    bash: echo "deploy ${account}"
"#;

    let tasks = parse_config(config, vec!["deploy", "--account", "work"]).unwrap();

    assert_eq!(find_task(&tasks, "deploy").envs.get("account").unwrap(), "work");
    assert_eq!(
        find_task(&tasks, "build").envs.get("account").unwrap(),
        "work",
        "build should inherit from deploy via 'after' relationship"
    );
}

// =============================================================================
// Error message quality
// =============================================================================

#[test]
#[serial]
fn test_diamond_conflict_error_names_tasks_and_values() {
    let config = r#"
otto:
  api: 1
  tasks: [alpha, beta]

tasks:
  shared:
    params:
      --target:
        default: local
        help: Target environment
    bash: echo "shared ${target}"

  alpha:
    params:
      --target:
        default: local
        help: Target environment
    before: [shared]
    bash: echo "alpha ${target}"

  beta:
    params:
      --target:
        default: local
        help: Target environment
    before: [shared]
    bash: echo "beta ${target}"
"#;

    let result = parse_config(config, vec!["alpha", "--target", "dev", "beta", "--target", "prod"]);

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("alpha"), "Error should name source task alpha: {}", err);
    assert!(err.contains("beta"), "Error should name source task beta: {}", err);
    assert!(err.contains("target"), "Error should name the param: {}", err);
    assert!(err.contains("dev"), "Error should show value 'dev': {}", err);
    assert!(err.contains("prod"), "Error should show value 'prod': {}", err);
}

#[test]
#[serial]
fn test_choices_error_names_source_and_target() {
    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  build:
    params:
      --env:
        default: dev
        choices: [dev, staging]
        help: Build environment
    bash: echo "build ${env}"

  deploy:
    params:
      --env:
        default: dev
        choices: [dev, staging, prod]
        help: Deploy environment
    before: [build]
    bash: echo "deploy ${env}"
"#;

    let result = parse_config(config, vec!["deploy", "--env", "prod"]);

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("deploy"), "Error should name source task: {}", err);
    assert!(err.contains("build"), "Error should name target task: {}", err);
    assert!(err.contains("prod"), "Error should show the invalid value: {}", err);
    assert!(err.contains("dev"), "Error should show allowed choices: {}", err);
    assert!(err.contains("staging"), "Error should show allowed choices: {}", err);
}
