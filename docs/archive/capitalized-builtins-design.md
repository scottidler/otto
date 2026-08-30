# Capitalized Built-in Commands Design

## Problem Statement

Otto's built-in commands (`clean`, `history`, `stats`, `graph`, `convert`) occupy namespace that users commonly want for their own automation tasks. This creates conflicts when users want to define tasks with common names like:

- `test` - common testing task name
- `build` - common build task name
- `clean` - users may want custom cleanup logic
- `stats` - users may want project-specific statistics
- `history` - users may want git history or custom history views

### Current Issues

1. **Namespace Pollution**: Built-in commands prevent users from using these common task names
2. **Missing from Help**: `convert` command is not shown in help output despite being implemented
3. **Scattered Definitions**: Built-in command lists are hardcoded in multiple locations:
   - `src/cli/parser.rs:860` - help builder array
   - `src/main.rs:41-70` - early routing match arms
   - `src/main.rs:263,322` - execution filtering
   - `src/cli/parser.rs:1276-1281` - injection function calls

4. **Maintenance Risk**: Adding new built-ins requires updating 4+ locations

## Proposed Solution

**Capitalize all built-in commands** to create a clear visual and namespace distinction:

```bash
# Built-in commands (capitalized)
otto Stats
otto History
otto Clean
otto Graph
otto Convert

# User-defined tasks (lowercase, any name)
otto test
otto build
otto deploy
otto stats        # User's custom stats - no conflict!
otto clean-cache  # User's custom clean - no conflict!
```

### Design Principles

1. **Zero Namespace Collision**: Users can use any lowercase/mixed-case names
2. **Self-Documenting**: Capital letter signals "this is a built-in system command"
3. **Case-Sensitive Matching**: `Stats` ≠ `stats` (works on all platforms)
4. **Single Source of Truth**: One constant defining all built-ins
5. **No Transition Period**: Clean break, version bump with changelog

## Technical Design

### Central Constants Definition

**New file**: `src/cli/builtins.rs`

```rust
/// Built-in command names (capitalized)
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
pub const BUILTIN_COMMANDS: &[&str] = &[
    "Clean",
    "Convert",
    "Graph",
    "History",
    "Stats",
];

/// Check if a command name is a built-in
pub fn is_builtin(name: &str) -> bool {
    BUILTIN_COMMANDS.contains(&name)
}
```

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                          main.rs                            │
│  Early Routing: Clean, Convert, History, Stats              │
│  (Commands that don't need ottofile parsing)                │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ▼
         ┌────────────────┐
         │   Parser::new   │
         │  Parse ottofile │
         └────────┬────────┘
                  │
                  ▼
    ┌─────────────────────────────┐
    │ inject_builtin_commands()   │
    │  - inject_clean_meta_task   │
    │  - inject_convert_meta_task │◄─── NEW
    │  - inject_graph_meta_task   │
    │  - inject_history_meta_task │
    │  - inject_stats_meta_task   │
    └────────┬────────────────────┘
             │
             ▼
    ┌────────────────────────┐
    │  build_help_command()  │
    │  Separate built-ins    │
    │  from user tasks       │
    └────────┬───────────────┘
             │
             ▼
    ┌────────────────────────┐
    │  Parser::parse()       │
    │  Returns tasks to run  │
    └────────┬───────────────┘
             │
             ▼
    ┌──────────────────────────────┐
    │  execute_with_*_output()     │
    │  Filter out built-ins        │
    │  Route Graph to visualizer   │
    └──────────────────────────────┘
```

## Implementation Plan

### Step 1: Create Central Constants ✅

**File**: `src/cli/builtins.rs`

```rust
//! Built-in command definitions and utilities

/// All built-in Otto commands (capitalized to avoid namespace conflicts)
pub const BUILTIN_COMMANDS: &[&str] = &["Clean", "Convert", "Graph", "History", "Stats"];

/// Check if a command name is a built-in
pub fn is_builtin(name: &str) -> bool {
    BUILTIN_COMMANDS.contains(&name)
}

/// Get help text explaining built-in naming convention
pub fn builtin_help() -> &'static str {
    "Built-in commands are capitalized (e.g., Stats, Clean) to avoid conflicts with user tasks."
}
```

**Update**: `src/cli/mod.rs`
```rust
pub mod builtins;
pub mod commands;
pub mod error;
pub mod macros;
pub mod parser;

pub use builtins::{BUILTIN_COMMANDS, is_builtin};
```

### Step 2: Update Parser - Rename Built-ins ✅

**File**: `src/cli/parser.rs`

#### 2.1 Update inject functions

Change all `inject_*_meta_task()` functions to use capitalized names:

```rust
fn inject_graph_meta_task(&mut self) {
    let graph_task = TaskSpec {
        name: "Graph".to_string(),  // Was: "graph"
        help: Some("[built-in] Visualize the task dependency graph".to_string()),
        // ... rest unchanged
    };
    self.config_spec.tasks.insert("Graph".to_string(), graph_task);
}

fn inject_clean_meta_task(&mut self) {
    let clean_task = TaskSpec {
        name: "Clean".to_string(),  // Was: "clean"
        help: Some("[built-in] Clean old runs from ~/.otto/".to_string()),
        // ... rest unchanged
    };
    self.config_spec.tasks.insert("Clean".to_string(), clean_task);
}

fn inject_history_meta_task(&mut self) {
    let history_task = TaskSpec {
        name: "History".to_string(),  // Was: "history"
        help: Some("[built-in] View execution history".to_string()),
        // ... rest unchanged
    };
    self.config_spec.tasks.insert("History".to_string(), history_task);
}

fn inject_stats_meta_task(&mut self) {
    let stats_task = TaskSpec {
        name: "Stats".to_string(),  // Was: "stats"
        help: Some("[built-in] View execution statistics".to_string()),
        // ... rest unchanged
    };
    self.config_spec.tasks.insert("Stats".to_string(), stats_task);
}
```

#### 2.2 Add inject_convert_meta_task (NEW)

```rust
fn inject_convert_meta_task(&mut self) {
    use crate::cfg::param::{Nargs, ParamType};

    let convert_task = TaskSpec {
        name: "Convert".to_string(),
        help: Some("[built-in] Convert Makefile to Otto YAML format".to_string()),
        after: vec![],
        before: vec![],
        input: vec![],
        output: vec![],
        envs: HashMap::new(),
        params: {
            let mut params = HashMap::new();

            params.insert(
                "strict".to_string(),
                ParamSpec {
                    name: "strict".to_string(),
                    short: None,
                    long: Some("strict".to_string()),
                    param_type: ParamType::FLG,
                    dest: None,
                    metavar: None,
                    default: None,
                    constant: Value::Empty,
                    choices: vec![],
                    nargs: Nargs::Zero,
                    help: Some("Treat warnings as errors".to_string()),
                    value: Value::Empty,
                },
            );

            params.insert(
                "output".to_string(),
                ParamSpec {
                    name: "output".to_string(),
                    short: Some('o'),
                    long: Some("output".to_string()),
                    param_type: ParamType::OPT,
                    dest: None,
                    metavar: Some("FILE".to_string()),
                    default: None,
                    constant: Value::Empty,
                    choices: vec![],
                    nargs: Nargs::One,
                    help: Some("Output file (default: stdout)".to_string()),
                    value: Value::Empty,
                },
            );

            params
        },
        action: "# Built-in convert command".to_string(),
    };

    self.config_spec.tasks.insert("Convert".to_string(), convert_task);
}
```

#### 2.3 Update inject_builtin_commands

```rust
fn inject_builtin_commands(&mut self) {
    self.inject_clean_meta_task();
    self.inject_convert_meta_task();  // NEW - was missing
    self.inject_graph_meta_task();
    self.inject_history_meta_task();
    self.inject_stats_meta_task();
}
```

#### 2.4 Update build_help_command

Replace hardcoded array with constant:

```rust
use crate::cli::builtins::BUILTIN_COMMANDS;

fn build_help_command(&self) -> Command {
    let mut cmd = Command::new("otto")
        // ... args ...
        .allow_external_subcommands(true);

    if !self.config_spec.tasks.is_empty() {
        // Separate regular tasks from built-in commands
        let mut regular_tasks: Vec<_> = self
            .config_spec
            .tasks
            .iter()
            .filter(|(name, _)| !BUILTIN_COMMANDS.contains(&name.as_str()))
            .collect();
        regular_tasks.sort_by_key(|(name, _)| name.as_str());

        for (_, task_spec) in regular_tasks {
            cmd = cmd.subcommand(Self::task_to_command(task_spec));
        }

        // Collect and sort built-in commands
        let mut builtins: Vec<(&String, &TaskSpec)> = self
            .config_spec
            .tasks
            .iter()
            .filter(|(name, _)| BUILTIN_COMMANDS.contains(&name.as_str()))
            .collect();
        builtins.sort_by_key(|(name, _)| name.as_str());

        for (_, task_spec) in builtins {
            cmd = cmd.subcommand(Self::task_to_command(task_spec));
        }
    } else {
        cmd = cmd.after_help(ottofile_not_found_message());
    }

    cmd
}
```

### Step 3: Update Main - Early Routing ✅

**File**: `src/main.rs`

#### 3.1 Update early routing match

```rust
use otto::cli::builtins::is_builtin;

#[tokio::main]
async fn main() {
    // ... logging setup ...

    let args: Vec<String> = env::args().collect();

    if args.len() > 1 {
        match args[1].as_str() {
            "Clean" => {  // Was: "clean"
                if let Err(e) = execute_clean_command(&args[1..]).await {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                return;
            }
            "Convert" => {  // Was: "convert"
                if let Err(e) = execute_convert_command(&args[1..]) {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                return;
            }
            "History" => {  // Was: "history"
                if let Err(e) = execute_history_command(&args[1..]) {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                return;
            }
            "Stats" => {  // Was: "stats"
                if let Err(e) = execute_stats_command(&args[1..]) {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                return;
            }
            _ => {}
        }
    }

    // ... rest of main ...
}
```

#### 3.2 Update execution filters

```rust
async fn execute_with_terminal_output(
    tasks: Vec<otto::cli::parser::Task>,
    hash: String,
    ottofile_path: Option<std::path::PathBuf>,
    jobs: usize,
) -> Result<(), Report> {
    if tasks.is_empty() {
        println!("No tasks to execute");
        return Ok(());
    }

    let clean_tasks: Vec<_> = tasks.iter().filter(|task| task.name == "Clean").collect();
    if !clean_tasks.is_empty() {
        return execute_clean_from_task(clean_tasks[0]).await;
    }

    let graph_tasks: Vec<_> = tasks.iter().filter(|task| task.name == "Graph").collect();
    if !graph_tasks.is_empty() {
        return DagVisualizer::execute_command(graph_tasks[0]).await;
    }

    let history_tasks: Vec<_> = tasks.iter().filter(|task| task.name == "History").collect();
    if !history_tasks.is_empty() {
        return execute_history_from_task(history_tasks[0]);
    }

    let stats_tasks: Vec<_> = tasks.iter().filter(|task| task.name == "Stats").collect();
    if !stats_tasks.is_empty() {
        return execute_stats_from_task(stats_tasks[0]);
    }

    // Filter out built-in commands for normal execution
    use otto::cli::builtins::is_builtin;
    let execution_tasks: Vec<_> = tasks
        .into_iter()
        .filter(|task| !is_builtin(&task.name))
        .collect();

    if execution_tasks.is_empty() {
        println!("No tasks to execute");
        return Ok(());
    }

    // ... rest of execution ...
}
```

```rust
async fn execute_with_tui_output(
    tasks: Vec<otto::cli::parser::Task>,
    hash: String,
    ottofile_path: Option<std::path::PathBuf>,
    jobs: usize,
) -> Result<(), Report> {
    use otto::tui::{TaskPane, TuiApp};

    if tasks.is_empty() {
        eprintln!("No tasks to execute");
        return Ok(());
    }

    // Filter out built-in commands for normal execution
    use otto::cli::builtins::is_builtin;
    let execution_tasks: Vec<_> = tasks
        .into_iter()
        .filter(|task| !is_builtin(&task.name))
        .collect();

    if execution_tasks.is_empty() {
        eprintln!("No tasks to execute");
        return Ok(());
    }

    // ... rest of TUI execution ...
}
```

### Step 4: Update Tests ✅

**File**: `tests/builtin_commands_test.rs`

Update all test cases to use capitalized names:

```rust
#[test]
fn test_builtin_graph_command_in_help() {
    // ... setup ...
    assert!(help_output.contains("Graph"));  // Was: "graph"
    assert!(help_output.contains("[built-in] Visualize"));
}

#[test]
fn test_builtin_clean_command_in_help() {
    // ... setup ...
    assert!(help_output.contains("Clean"));  // Was: "clean"
    assert!(help_output.contains("[built-in] Clean old runs"));
}

#[test]
fn test_builtin_history_command_in_help() {
    // ... setup ...
    assert!(help_output.contains("History"));  // Was: "history"
    assert!(help_output.contains("[built-in] View execution history"));
}

#[test]
fn test_builtin_stats_command_in_help() {
    // ... setup ...
    assert!(help_output.contains("Stats"));  // Was: "stats"
    assert!(help_output.contains("[built-in] View execution statistics"));
}

#[test]
fn test_builtin_convert_command_in_help() {
    // NEW TEST
    let temp = TempDir::new().unwrap();
    let ottofile = temp.path().join("otto.yml");
    write(&ottofile, "tasks:\n  test:\n    action: echo test\n").unwrap();

    let output = Command::new(get_otto_binary())
        .current_dir(temp.path())
        .arg("--help")
        .output()
        .expect("Failed to execute otto");

    let help_output = String::from_utf8(output.stdout).unwrap();
    assert!(help_output.contains("Convert"));
    assert!(help_output.contains("[built-in] Convert Makefile"));
}
```

### Step 5: Update Documentation ✅

#### 5.1 Update README.md

```markdown
## Built-in Commands

Otto provides several built-in system commands (capitalized):

- `otto Stats` - View execution statistics
- `otto History` - View execution history
- `otto Clean` - Clean old runs from ~/.otto/
- `otto Graph` - Visualize task dependency graph
- `otto Convert` - Convert Makefile to Otto YAML

User-defined tasks use lowercase or mixed-case names to avoid conflicts.
```

#### 5.2 Update command documentation

**File**: `docs/commands/stats.md`

```markdown
# Stats Command

View execution statistics for tasks.

## Usage

\`\`\`bash
otto Stats [OPTIONS]
\`\`\`

Note: Built-in commands are capitalized to avoid namespace conflicts with user tasks.
```

**File**: `docs/commands/history.md`

```markdown
# History Command

View execution history for tasks.

## Usage

\`\`\`bash
otto History [OPTIONS]
\`\`\`

Note: Built-in commands are capitalized to avoid namespace conflicts with user tasks.
```

**File**: `docs/commands/clean.md`

```markdown
# Clean Command

Clean old runs from ~/.otto/ directory.

## Usage

\`\`\`bash
otto Clean [OPTIONS]
\`\`\`

Note: Built-in commands are capitalized to avoid namespace conflicts with user tasks.
```

#### 5.3 Create new documentation

**File**: `docs/commands/convert.md`

```markdown
# Convert Command

Convert Makefile to Otto YAML format.

## Usage

\`\`\`bash
cat Makefile | otto Convert [OPTIONS]
\`\`\`

## Options

- `--strict` - Treat warnings as errors
- `-o, --output <FILE>` - Output file (default: stdout)

## Examples

\`\`\`bash
# Convert to stdout
cat Makefile | otto Convert

# Convert to file
cat Makefile | otto Convert -o otto.yml

# Strict mode
cat Makefile | otto Convert --strict
\`\`\`

## Notes

- Reads from stdin
- Outputs YAML format
- Built-in commands are capitalized to avoid namespace conflicts with user tasks
```

**File**: `docs/commands/graph.md`

```markdown
# Graph Command

Visualize the task dependency graph.

## Usage

\`\`\`bash
otto Graph [OPTIONS]
\`\`\`

## Options

- `-f, --format <FORMAT>` - Output format: ascii, dot, svg, png, pdf (default: ascii)
- `--output <FILE>` - Output file path

## Examples

\`\`\`bash
# ASCII art to terminal
otto Graph

# DOT format
otto Graph --format dot

# Generate SVG
otto Graph --format svg --output graph.svg
\`\`\`

## Notes

- Built-in commands are capitalized to avoid namespace conflicts with user tasks
```

#### 5.4 Update migration guide

**File**: `docs/migration-guide.md`

Add new section:

```markdown
## Breaking Changes: Capitalized Built-ins

**Version**: 0.x.0 → 0.y.0

### What Changed

All built-in commands are now capitalized to avoid namespace conflicts with user-defined tasks.

**Old (v0.x)**:
\`\`\`bash
otto stats
otto history
otto clean
otto graph
otto convert
\`\`\`

**New (v0.y+)**:
\`\`\`bash
otto Stats
otto History
otto Clean
otto Graph
otto Convert
\`\`\`

### Why?

Users frequently want to use common task names like `test`, `build`, `clean`, `stats` for their own automation. Capitalizing built-ins creates a clear namespace separation:

- **Built-ins**: Capitalized (e.g., `Stats`, `Clean`)
- **User tasks**: Any case (e.g., `test`, `build`, `my-task`)

### Migration

1. Update any scripts or CI/CD pipelines that use built-in commands
2. Use find/replace to update capitalization:
   - `otto stats` → `otto Stats`
   - `otto history` → `otto History`
   - `otto clean` → `otto Clean`
   - `otto graph` → `otto Graph`
   - `otto convert` → `otto Convert`

### Benefit

You can now define tasks with names that were previously reserved:

\`\`\`yaml
# otto.yml - Now valid!
tasks:
  stats:
    help: "Generate project statistics"
    action: |
      #!/usr/bin/env bash
      echo "Custom stats command"

  clean:
    help: "Clean project artifacts"
    action: |
      #!/usr/bin/env bash
      rm -rf build/ dist/
\`\`\`
```

## Implementation Checklist

### Phase 1: Core Changes ✅
- [ ] Create `src/cli/builtins.rs` with `BUILTIN_COMMANDS` constant
- [ ] Export from `src/cli/mod.rs`
- [ ] Update all `inject_*_meta_task()` functions to use capitalized names
- [ ] Add `inject_convert_meta_task()` function
- [ ] Update `inject_builtin_commands()` to include Convert
- [ ] Update `build_help_command()` to use `BUILTIN_COMMANDS` constant

### Phase 2: Routing Updates ✅
- [ ] Update early routing in `main.rs` to use capitalized names
- [ ] Update `execute_with_terminal_output()` filters
- [ ] Update `execute_with_tui_output()` filters
- [ ] Use `is_builtin()` helper for cleaner filtering

### Phase 3: Testing ✅
- [ ] Update `tests/builtin_commands_test.rs` for all capitalized names
- [ ] Add test for Convert command in help
- [ ] Test case sensitivity (ensure `stats` doesn't trigger `Stats`)
- [ ] Test user can define lowercase `stats`, `clean`, etc.
- [ ] Run full test suite

### Phase 4: Documentation ✅
- [ ] Update `README.md` with capitalized examples
- [ ] Update `docs/commands/stats.md`
- [ ] Update `docs/commands/history.md`
- [ ] Update `docs/commands/clean.md`
- [ ] Create `docs/commands/convert.md`
- [ ] Create `docs/commands/graph.md`
- [ ] Update `docs/migration-guide.md` with breaking change notice

### Phase 5: Examples & Polish ✅
- [ ] Update any example scripts in `examples/`
- [ ] Update `CHANGELOG.md` with breaking change
- [ ] Bump version number in `Cargo.toml`
- [ ] Update any shell completion scripts

## Testing Strategy

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_commands_are_capitalized() {
        for cmd in BUILTIN_COMMANDS {
            assert!(cmd.chars().next().unwrap().is_uppercase());
        }
    }

    #[test]
    fn test_is_builtin() {
        assert!(is_builtin("Stats"));
        assert!(is_builtin("Clean"));
        assert!(is_builtin("Graph"));
        assert!(is_builtin("History"));
        assert!(is_builtin("Convert"));

        // Lowercase should NOT match
        assert!(!is_builtin("stats"));
        assert!(!is_builtin("clean"));

        // Random names should NOT match
        assert!(!is_builtin("test"));
        assert!(!is_builtin("build"));
    }
}
```

### Integration Tests

```rust
#[test]
fn test_user_can_define_lowercase_stats_task() {
    let temp = TempDir::new().unwrap();
    let ottofile = temp.path().join("otto.yml");

    // User defines lowercase "stats" task
    write(&ottofile, r#"
tasks:
  stats:
    help: "Custom stats"
    action: echo "User stats"
"#).unwrap();

    let output = Command::new(get_otto_binary())
        .current_dir(temp.path())
        .arg("stats")
        .output()
        .expect("Failed to execute otto");

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("User stats"));
    assert!(!stdout.contains("Statistics"));  // Built-in would show this
}

#[test]
fn test_builtin_stats_with_capital() {
    let temp = TempDir::new().unwrap();
    let ottofile = temp.path().join("otto.yml");
    write(&ottofile, "tasks: {}\n").unwrap();

    let output = Command::new(get_otto_binary())
        .current_dir(temp.path())
        .arg("Stats")
        .output()
        .expect("Failed to execute otto");

    // Should execute built-in Stats command
    assert!(output.status.success());
}
```

### Manual Testing Checklist

- [ ] `otto --help` shows all capitalized built-ins
- [ ] `otto Stats` works
- [ ] `otto History` works
- [ ] `otto Clean --keep 7` works
- [ ] `otto Graph --format ascii` works
- [ ] `cat Makefile | otto Convert` works
- [ ] User can define `stats:` task in ottofile without conflict
- [ ] User can define `clean:` task in ottofile without conflict
- [ ] `otto stats` runs user's task, not built-in
- [ ] Case sensitivity works on Linux/macOS
- [ ] Case sensitivity works on Windows (preserves case)

## Rollout Plan

### Version Bump
- Current: `v0.x.y`
- Target: `v0.X.0` (minor version bump for breaking change)

### Changelog Entry

```markdown
## [0.X.0] - 2025-11-XX

### BREAKING CHANGES

- **Built-in commands are now capitalized** to avoid namespace conflicts with user tasks
  - `stats` → `Stats`
  - `history` → `History`
  - `clean` → `Clean`
  - `graph` → `Graph`
  - `convert` → `Convert`
- Users can now define tasks with lowercase names that were previously reserved
- Update any scripts using built-in commands to use capitalized versions

### Added

- `Convert` command now appears in help output
- Centralized built-in command definitions in `src/cli/builtins.rs`

### Fixed

- `convert` command missing from help output
- Inconsistent built-in command handling across codebase
```

### Communication

1. **GitHub Release Notes**: Include migration examples
2. **README**: Update all examples immediately
3. **Documentation Site**: Update all command references

## Benefits

### For Users

1. **Freedom to name tasks**: Can use `test`, `build`, `stats`, `clean`, etc.
2. **Clear distinction**: Capital = system command, lowercase = user task
3. **No conflicts**: Built-ins never collide with user tasks
4. **Predictable**: Convention is simple and consistent

### For Maintainers

1. **Single source of truth**: `BUILTIN_COMMANDS` constant
2. **Easier to add built-ins**: One place to update
3. **Type safety**: `is_builtin()` helper prevents typos
4. **Cleaner code**: Less duplication across modules

## Future Considerations

### Potential New Built-ins

With namespace freed up, consider adding:

- `otto Init` - Initialize new ottofile
- `otto Doctor` - Check system dependencies
- `otto Config` - Manage otto configuration
- `otto Upgrade` - Self-update otto
- `otto Validate` - Validate ottofile syntax

All would follow the capitalization convention.

### Shell Completion

Update shell completion scripts to suggest capitalized built-ins:

```bash
# bash/zsh completion
_otto_completions() {
    local builtins="Clean Convert Graph History Stats"
    # ... completion logic ...
}
```

### IDE Integration

IDEs/LSPs could highlight built-in commands differently:
- Capitalized built-ins: system color (blue)
- User tasks: custom color (green)

## Success Criteria

1. ✅ All built-in commands use consistent capitalization
2. ✅ Single source of truth for built-in list
3. ✅ `Convert` appears in help output
4. ✅ Users can define tasks with any previously-reserved names
5. ✅ All tests pass
6. ✅ Documentation updated
7. ✅ Migration guide provided
8. ✅ No regression in functionality

## Timeline Estimate

- **Phase 1-2 (Core + Routing)**: 2-3 hours
- **Phase 3 (Testing)**: 2-3 hours
- **Phase 4 (Documentation)**: 2-3 hours
- **Phase 5 (Polish)**: 1-2 hours

**Total**: 7-11 hours of development time

## References

- Original discussion: Architecture planning session
- Related issue: `convert` missing from help
- Current built-in locations:
  - `src/cli/parser.rs` - Injection and help builder
  - `src/main.rs` - Early routing and execution filtering
  - `src/cli/commands/` - Command implementations
