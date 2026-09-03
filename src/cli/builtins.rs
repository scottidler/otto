//! Built-in command definitions and utilities

/// All built-in Otto commands (capitalized to avoid namespace conflicts)
///
/// These commands are system-level operations that don't require an ottofile
/// or operate on otto's internal state/database.
///
/// IMPORTANT: When adding a new built-in, every one of these has to change,
/// because each list is consulted by a different surface and a builtin missing
/// from any one of them fails silently rather than loudly:
/// 1. Add the name to `BUILTIN_COMMANDS` below - this is what reserves the
///    name against a user task (`Parser::validate_no_builtin_tasks`) and what
///    keeps it out of `*` expansion and `--tasks`.
/// 2. Add a variant to the `Builtin` enum (`app.rs`) and its `task_name()` and
///    `all()` arms; `dispatch_builtin` is an exhaustive match, so the compiler
///    then demands the handler.
/// 3. Add the command's clap `Command` and its `[built-in]` help line to
///    `builtin_clap_commands()` (`cli/parser/meta_tasks.rs`), so the name has
///    a `TaskSpec` derived from that `Command` and appears in `--help`. Its
///    params come from the derive; there is no second list of flags to write.
/// 4. Add the execution handler `app.rs` dispatches to.
/// 5. Add a variant to `EarlyCommand` (`main.rs`) if it can run without an
///    ottofile, so `otto NAME --flags` reaches its own clap parser before
///    discovery. `Graph` is the one builtin that must NOT be there: it needs
///    the parsed ottofile's task specs, so it is reached by the task route
///    alone (pinned by `every_builtin_but_graph_is_early_routed`).
///
/// There is no execution filter to update: a task list mixing a builtin with a
/// user task is rejected in `Parser::parse` instead of being filtered down.
pub const BUILTIN_COMMANDS: &[&str] = &["Clean", "Convert", "Graph", "History", "Stats", "Upgrade"];

/// Check if a command name is a built-in
pub fn is_builtin(name: &str) -> bool {
    BUILTIN_COMMANDS.contains(&name)
}

/// Built-in params that are auto-injected on certain tasks (capitalized)
///
/// These params are automatically added by otto and cannot be defined by users.
/// - Serial: Auto-injected on foreach tasks to allow sequential execution
pub const BUILTIN_PARAMS: &[&str] = &["Serial"];

/// Check if a param name is reserved for builtins
pub fn is_builtin_param(name: &str) -> bool {
    BUILTIN_PARAMS.contains(&name)
}

#[path = "builtins_tests.rs"]
mod tests;
