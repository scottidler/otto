//! Built-in command definitions and utilities

/// All built-in Otto commands (capitalized to avoid namespace conflicts)
///
/// These commands are system-level operations that don't require an ottofile
/// or operate on otto's internal state/database.
///
/// IMPORTANT: When adding a new built-in:
/// 1. Add name to this array
/// 2. Create inject_NAME_meta_task() in parser.rs
/// 3. Add early routing in main.rs if it doesn't need ottofile
/// 4. Add execution filter if it shouldn't run as normal task
/// 5. Add execution handler function
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
