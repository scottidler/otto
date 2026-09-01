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
//! parser call sites keep working) and `run_command_stdout` owns the one
//! execution contract every dynamic source shares: `sh -c`, cwd = the
//! ottofile's directory, env = inherited environment + resolved global
//! `envs:`, plus a recursion guard variable so a nested otto cannot resolve
//! the same source again. `run_lines_command` is the trim-and-filter wrapper
//! over it that foreach items and dynamic choices want.
//!
//! `otto.envs-command` shares that execution contract but NOT these caches:
//! it resolves inside the `global_envs` initializer, which sits upstream of
//! every `RefCell` here, and re-entering the `OnceCell` from its own
//! initializer would panic. The `OnceCell` already gives it once-per-invocation.

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

/// Environment variable naming the `otto.envs-command` currently being
/// resolved. Symmetric with the other two, except that there is exactly one
/// resolution per invocation, so the guard *key* is a fixed literal rather
/// than a task name.
pub const ENVS_GUARD_VAR: &str = "OTTO_ENVS_COMMAND";

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
    /// The failure is memoized too, as its message: an env evaluation that
    /// failed once fails the same way every time, and every asking surface has
    /// to see it rather than the first one seeing an error and the rest an
    /// empty map.
    global_envs: OnceCell<std::result::Result<HashMap<String, String>, String>>,
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
    ///
    /// Fails closed: if the globals cannot be evaluated, callers get the error.
    /// They used to get a warning on stderr and an empty map, so a circular or
    /// unresolvable global env silently removed every global from the run.
    pub fn global_envs<F>(&self, init: F) -> Result<&HashMap<String, String>>
    where
        F: FnOnce() -> Result<HashMap<String, String>>,
    {
        match self.global_envs.get_or_init(|| init().map_err(|e| e.to_string())) {
            Ok(envs) => Ok(envs),
            Err(message) => Err(eyre!("Failed to evaluate global environment variables: {}", message)),
        }
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
/// The trim-and-filter wrapper over [`run_command_stdout`], which owns the
/// shared execution contract. Foreach items and dynamic choices both want
/// trimmed, non-empty lines; `otto.envs-command` calls the raw form directly
/// because `KEY=  spaced value  ` must survive byte-for-byte.
pub fn run_lines_command(
    command: &str,
    cwd: &Path,
    envs: &HashMap<String, String>,
    guard_var: &str,
    guard_key: &str,
    context: &str,
) -> Result<Vec<String>> {
    let stdout = run_command_stdout(command, cwd, envs, guard_var, guard_key, context)?;
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect())
}

/// Run a dynamic source's command and return its stdout verbatim.
///
/// This is the one execution contract every command source shares: `sh -c`,
/// cwd = the ottofile's directory, the caller's env layered onto the inherited
/// one (no `env_clear`), a loud non-zero exit naming the command and its
/// trimmed stderr, and stderr passthrough on success so a generator's own
/// warnings stay visible while stdout stays pure data.
///
/// `context` prefixes every error (e.g. `foreach task 'up'`); `guard_var` /
/// `guard_key` implement the recursion guard: if `guard_var` already names
/// `guard_key`, this errors instead of recursing, and the child process sees
/// `guard_key` appended so an inner otto can make the same check.
pub fn run_command_stdout(
    command: &str,
    cwd: &Path,
    envs: &HashMap<String, String>,
    guard_var: &str,
    guard_key: &str,
    context: &str,
) -> Result<String> {
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

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[path = "resolver_tests.rs"]
mod tests;
