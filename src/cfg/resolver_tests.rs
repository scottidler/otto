#![cfg(test)]

use super::*;
use serial_test::serial;
use std::path::PathBuf;

fn no_envs() -> HashMap<String, String> {
    HashMap::new()
}

#[test]
fn run_lines_command_trims_and_drops_empty_lines() {
    let lines = run_lines_command(
        "printf 'alpha\\n\\n  beta  \\n'",
        &PathBuf::from("."),
        &no_envs(),
        FOREACH_GUARD_VAR,
        "up",
        "foreach task 'up'",
    )
    .unwrap();
    assert_eq!(lines, vec!["alpha".to_string(), "beta".to_string()]);
}

#[test]
fn run_lines_command_reports_nonzero_exit_with_command_and_context() {
    let err = run_lines_command(
        "echo boom >&2; exit 3",
        &PathBuf::from("."),
        &no_envs(),
        FOREACH_GUARD_VAR,
        "up",
        "foreach task 'up'",
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("foreach task 'up'"), "{err}");
    assert!(err.contains("echo boom >&2; exit 3"), "{err}");
    assert!(err.contains("exit code 3"), "{err}");
    assert!(err.contains("boom"), "{err}");
}

#[test]
fn run_lines_command_sees_the_supplied_envs_and_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let mut envs = HashMap::new();
    envs.insert("FOREACH_TEST_VAR".to_string(), "from-globals".to_string());
    let lines = run_lines_command(
        "printf '%s\\n' \"$FOREACH_TEST_VAR\"; pwd",
        dir.path(),
        &envs,
        FOREACH_GUARD_VAR,
        "up",
        "foreach task 'up'",
    )
    .unwrap();
    assert_eq!(lines[0], "from-globals");
    let reported = std::fs::canonicalize(&lines[1]).unwrap();
    assert_eq!(reported, std::fs::canonicalize(dir.path()).unwrap());
}

#[test]
fn foreach_items_resolves_once_and_caches() {
    let resolver = DynamicResolver::new();
    let calls = std::cell::Cell::new(0);
    let resolve = || {
        calls.set(calls.get() + 1);
        Ok(vec![ForeachItem {
            identifier: "alpha".to_string(),
            value: "alpha".to_string(),
        }])
    };
    let first = resolver.foreach_items("up", resolve).unwrap();
    let second = resolver.foreach_items("up", resolve).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        calls.get(),
        1,
        "command source must resolve at most once per invocation"
    );
    assert!(resolver.has_foreach("up"));
    assert!(!resolver.has_foreach("down"));
}

#[test]
fn foreach_items_does_not_cache_a_failure() {
    let resolver = DynamicResolver::new();
    assert!(resolver.foreach_items("up", || Err(eyre!("boom"))).is_err());
    assert!(!resolver.has_foreach("up"));
    let items = resolver.foreach_items("up", || Ok(Vec::new())).unwrap();
    assert!(items.is_empty());
}

#[test]
fn global_envs_initializes_once() {
    let resolver = DynamicResolver::new();
    let calls = std::cell::Cell::new(0);
    for _ in 0..3 {
        let _ = resolver.global_envs(|| {
            calls.set(calls.get() + 1);
            Ok(HashMap::new())
        });
    }
    assert_eq!(calls.get(), 1);
}

#[test]
fn choices_resolves_once_and_caches() {
    let resolver = DynamicResolver::new();
    let calls = std::cell::Cell::new(0);
    let resolve = || {
        calls.set(calls.get() + 1);
        Ok(vec!["alpha".to_string()])
    };
    // The two bind triggers (direct invocation, propagation validation) ask
    // for the same key; only the first may execute.
    let first = resolver.choices("switch:svc", resolve).unwrap();
    let second = resolver.choices("switch:svc", resolve).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        calls.get(),
        1,
        "a choices command must resolve at most once per invocation"
    );
    assert!(resolver.has_choices("switch:svc"));
    assert!(!resolver.has_choices("switch:other"));
}

#[test]
fn choices_does_not_cache_a_failure() {
    let resolver = DynamicResolver::new();
    assert!(resolver.choices("switch:svc", || Err(eyre!("boom"))).is_err());
    assert!(!resolver.has_choices("switch:svc"));
}

#[test]
fn run_lines_command_refuses_to_recurse_on_the_same_choices_key() {
    // Stands in for a nested otto: the guard var already names this key.
    unsafe { std::env::set_var(CHOICES_GUARD_VAR, "switch:svc") };
    let err = run_lines_command(
        "printf 'alpha\n'",
        &PathBuf::from("."),
        &no_envs(),
        CHOICES_GUARD_VAR,
        "switch:svc",
        "Task 'switch' param 'svc' choices-command",
    )
    .unwrap_err()
    .to_string();
    unsafe { std::env::remove_var(CHOICES_GUARD_VAR) };
    assert!(err.contains("recursion detected"), "{err}");
    assert!(err.contains("switch:svc"), "{err}");
    assert!(err.contains(CHOICES_GUARD_VAR), "{err}");
}

// ----------------------------------------------------------------------
// `run_command_stdout`: the raw form `otto.envs-command` uses.
// ----------------------------------------------------------------------

/// The whole point of the split: the raw form keeps whitespace and blank
/// lines that `run_lines_command` trims and drops, so a `KEY=  spaced  ` pair
/// survives byte-for-byte.
#[test]
#[serial]
fn run_command_stdout_returns_output_verbatim() {
    let stdout = run_command_stdout(
        "printf 'alpha\\n\\n  beta  \\n'",
        &PathBuf::from("."),
        &no_envs(),
        ENVS_GUARD_VAR,
        "otto.envs-command",
        "otto.envs-command",
    )
    .unwrap();
    assert_eq!(stdout, "alpha\n\n  beta  \n");
}

/// Same exit-code contract as the wrapper: loud, naming the context, the
/// command, the code, and the command's own stderr.
#[test]
#[serial]
fn run_command_stdout_reports_nonzero_exit_with_command_and_context() {
    let err = run_command_stdout(
        "echo boom >&2; exit 3",
        &PathBuf::from("."),
        &no_envs(),
        ENVS_GUARD_VAR,
        "otto.envs-command",
        "otto.envs-command",
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("otto.envs-command"), "{err}");
    assert!(err.contains("exit code 3"), "{err}");
    assert!(err.contains("boom"), "{err}");
}

/// The guard chain is the same mechanism, with a fixed literal key rather
/// than a task name: there is one `envs-command` resolution per invocation.
#[test]
#[serial]
fn run_command_stdout_refuses_to_recurse_on_the_envs_command_key() {
    // Stands in for a nested otto: the guard var already names the key.
    unsafe { std::env::set_var(ENVS_GUARD_VAR, "otto.envs-command") };
    let err = run_command_stdout(
        "printf 'FOO=bar\n'",
        &PathBuf::from("."),
        &no_envs(),
        ENVS_GUARD_VAR,
        "otto.envs-command",
        "otto.envs-command",
    )
    .unwrap_err()
    .to_string();
    unsafe { std::env::remove_var(ENVS_GUARD_VAR) };
    assert!(err.contains("recursion detected"), "{err}");
    assert!(err.contains("otto.envs-command"), "{err}");
    assert!(err.contains(ENVS_GUARD_VAR), "{err}");
}
