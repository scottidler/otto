//! Per-invocation resolution cache for dynamic (command-sourced) config values.
//!
//! Otto's static config sources (`foreach: glob/items/range`, static param
//! `choices`) are pure functions of the ottofile and are resolved wherever they
//! are needed. A *command* source executes, so it gets three extra rules
//! (docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md, Phase 6):
//!
//! - **Lazy:** it runs only when the invocation actually needs the expansion.
//!   `otto --help` never runs it; targeting an unrelated task never runs it.
//! - **Cached:** at most once per otto invocation, no matter how many call
//!   sites ask for the same value.
//! - **Loud:** a non-zero exit is a config error naming the task and the
//!   command, never a silent empty expansion.
//!
//! `DynamicResolver` owns the caches (interior mutability, so the `&self`
//! parser call sites keep working) and `run_lines_command` owns the one
//! execution contract every dynamic source shares: `sh -c`, cwd = the
//! ottofile's directory, env = inherited environment + resolved global
//! `envs:`, plus a recursion guard variable so a nested otto cannot resolve
//! the same source again.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use eyre::{Result, eyre};

use crate::cfg::task::ForeachItem;

/// Environment variable naming the foreach task(s) currently being resolved.
/// Read by a nested otto to refuse an infinite resolution cycle.
pub const FOREACH_GUARD_VAR: &str = "OTTO_FOREACH_COMMAND";

/// Environment variable naming the `task:param` choices source(s) currently
/// being resolved. Symmetric with `FOREACH_GUARD_VAR`.
pub const CHOICES_GUARD_VAR: &str = "OTTO_CHOICES_COMMAND";

/// Per-invocation cache for everything a dynamic source resolves.
///
/// Keyed by task name for foreach, by `task:param` for dynamic param choices -
/// the point of this type is that both share one cache lifetime (the
/// invocation) and one execution contract.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DynamicResolver {
    /// Resolved global `envs:`, evaluated at most once and only when a dynamic
    /// source actually needs them (they are part of the command's environment
    /// contract, so they must be resolved before any command runs).
    global_envs: OnceCell<HashMap<String, String>>,
    /// task name -> resolved items, for command-sourced foreach only.
    foreach: RefCell<HashMap<String, Vec<ForeachItem>>>,
    /// `task:param` -> resolved value set, for `choices-command` params only.
    choices: RefCell<HashMap<String, Vec<String>>>,
}

impl DynamicResolver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolved global `envs:`, computed by `init` on first call and reused after.
    pub fn global_envs<F>(&self, init: F) -> &HashMap<String, String>
    where
        F: FnOnce() -> HashMap<String, String>,
    {
        self.global_envs.get_or_init(init)
    }

    /// Cached foreach items for `task`, resolved by `resolve` on the first ask.
    ///
    /// A failure is not cached: it is returned to every caller, which is what
    /// keeps a resolution failure loud at whichever surface asked first.
    pub fn foreach_items<F>(&self, task: &str, resolve: F) -> Result<Vec<ForeachItem>>
    where
        F: FnOnce() -> Result<Vec<ForeachItem>>,
    {
        if let Some(items) = self.foreach.borrow().get(task) {
            return Ok(items.clone());
        }
        let items = resolve()?;
        self.foreach.borrow_mut().insert(task.to_string(), items.clone());
        Ok(items)
    }

    /// Whether `task`'s command source has already run this invocation.
    #[must_use]
    pub fn has_foreach(&self, task: &str) -> bool {
        self.foreach.borrow().contains_key(task)
    }

    /// Cached dynamic `choices` for `key` (`task:param`), resolved on first ask.
    ///
    /// This is what makes the two bind triggers (direct invocation and
    /// propagation validation) add up to one execution: whichever fires first
    /// pays for the command, the other reads the cache. As with foreach, a
    /// failure is not cached, so every asking surface sees it loudly.
    pub fn choices<F>(&self, key: &str, resolve: F) -> Result<Vec<String>>
    where
        F: FnOnce() -> Result<Vec<String>>,
    {
        if let Some(values) = self.choices.borrow().get(key) {
            return Ok(values.clone());
        }
        let values = resolve()?;
        self.choices.borrow_mut().insert(key.to_string(), values.clone());
        Ok(values)
    }

    /// Whether `key`'s (`task:param`) choices command has already run this invocation.
    #[must_use]
    pub fn has_choices(&self, key: &str) -> bool {
        self.choices.borrow().contains_key(key)
    }
}

/// Run a dynamic source's command and return its non-empty, trimmed stdout lines.
///
/// `context` prefixes every error (e.g. `foreach task 'up'`); `guard_var` /
/// `guard_key` implement the recursion guard: if `guard_var` already names
/// `guard_key`, this errors instead of recursing, and the child process sees
/// `guard_key` appended so an inner otto can make the same check.
pub fn run_lines_command(
    command: &str,
    cwd: &Path,
    envs: &HashMap<String, String>,
    guard_var: &str,
    guard_key: &str,
    context: &str,
) -> Result<Vec<String>> {
    let chain = std::env::var(guard_var).unwrap_or_default();
    if chain.split(',').any(|entry| entry == guard_key) {
        return Err(eyre!(
            "{context}: recursion detected resolving command '{command}' - \
             '{guard_key}' is already being resolved by an outer otto invocation \
             ({guard_var}={chain}); cycle: {chain} -> {guard_key}"
        ));
    }
    let nested = if chain.is_empty() {
        guard_key.to_string()
    } else {
        format!("{chain},{guard_key}")
    };

    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .envs(envs)
        .env(guard_var, nested)
        .output()
        .map_err(|e| eyre!("{context}: failed to run command '{command}': {e}"))?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let code = output
            .status
            .code()
            .map_or_else(|| "signal".to_string(), |c| c.to_string());
        let detail = if stderr.trim().is_empty() {
            String::new()
        } else {
            format!(": {}", stderr.trim())
        };
        return Err(eyre!(
            "{context}: command '{command}' failed with exit code {code}{detail}"
        ));
    }
    // The command succeeded; its diagnostics are still the user's to see, and
    // stdout must stay pure data.
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
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
            resolver.global_envs(|| {
                calls.set(calls.get() + 1);
                HashMap::new()
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
}
