# CLI Two-Pass Parser Design

## Overview

This document outlines the design plan for refactoring Otto's CLI parser to use a proper two-pass nom parser combinator approach, eliminating the current "disgusting" `main_nom()` function that uses manual string parsing instead of proper nom combinators.

## Current Problem

The existing `main_nom()` function in `src/main.rs` is a hybrid approach that defeats the purpose of using nom as a parser combinator library:

1. **Manual string parsing** before nom (lines 66-200+ in main.rs)
2. **Chicken-and-egg problem**: Need `--ottofile` to load config, but need config to validate tasks
3. **Bypasses nom parser**: Uses `split_whitespace()` and manual iteration instead of combinators
4. **Maintenance nightmare**: Complex nested if-blocks that are hard to extend

## Solution: Two-Pass Parsing with Config-Aware Task Segmentation

### Architecture Overview

```
Command Line Input
        ↓
   ┌─────────────┐
   │   Pass 1    │  ← Parse global options only (no config needed)
   │ Global Opts │  ← Pure nom combinators, restrictive parsing
   └─────────────┘
        ↓
   [Load Config]   ← Use --ottofile from Pass 1
        ↓           ← Extract known task names from config
   ┌─────────────┐
   │   Pass 2    │  ← Parse tasks with config-aware disambiguation
   │Config-Aware │  ← Task names become KEYWORDS that segment command line
   │Task Parsing │  ← Use config to resolve --flag vs --flag value ambiguity
   └─────────────┘
        ↓
    [Execute]
```

### Pass 1: Global Options Parser

**Purpose**: Parse only global options that don't require config knowledge.

**Input**: Full command line string
**Output**: `(GlobalOptions, remaining_args: &str)`

**Global Options to Parse**:
- `--ottofile` / `-o` - Path to config file
- `--help` / `-h` - Show help (short-circuits execution)
- `--version` / `-V` - Show version (short-circuits execution)
- `--api` / `-a` - API endpoint
- `--jobs` / `-j` - Number of parallel jobs
- `--home` / `-H` - Home directory
- `--tasks` / `-t` - Task filter
- `--verbosity` / `-v` - Verbosity level

**Example**:
```bash
# Input: "--ottofile custom.yml --verbose hello --greeting world test --flag"
# Pass 1 Output:
# - GlobalOptions { ottofile: Some("custom.yml"), verbosity: Some(1), ... }
# - Remaining: "hello --greeting world test --flag"
```

### Pass 2: Config-Aware Task Parser

**Purpose**: Parse task invocations using configuration to eliminate ambiguity.

**Input**:
- `remaining_args` from Pass 1
- `ConfigSpec` loaded using `--ottofile` from Pass 1

**Output**: `Vec<ParsedTask>`

**Key Innovation**: **Task names become KEYWORDS** that segment the command line:

1. **Extract known task names** from `ConfigSpec.tasks.keys()`
2. **Use task names as delimiters** to partition command line
3. **Resolve flag ambiguity** using config:
   - `--flag taskname` → if `taskname` is a known task, `--flag` is boolean
   - `--flag somevalue` → if `somevalue` is not a known task, `--flag` takes the value
4. **Validate arguments** against task parameter specifications

**What it parses**:
- Task names (validated against config.tasks)
- Task arguments (validated against task parameter specs)
- Task flags (disambiguated using known task names)

## Implementation Plan

### 1. Create `src/cli/global_options_parser.rs`

```rust
use nom::{IResult, combinator::all_consuming, multi::many0, sequence::tuple, branch::alt};
use crate::cli::types::{GlobalOption, ParseError};

pub type ParseResult<'a, T> = IResult<&'a str, T, ParseError>;

/// Parse only global options, return remaining input for Pass 2
pub fn parse_global_options_only(input: &str) -> ParseResult<(Vec<GlobalOption>, &str)> {
    let (remaining, (global_opts, _)) = tuple((
        many0(preceded(whitespace, global_option)),
        whitespace,
    ))(input)?;

    Ok((remaining, (global_opts, remaining)))
}

/// Global option parser (reuse existing combinators from combinators.rs)
pub fn global_option(input: &str) -> ParseResult<GlobalOption> {
    // Reuse existing global_option combinators
    crate::cli::combinators::global_option(input)
}

/// Parse everything that's NOT a global option as remaining args
pub fn remaining_args(input: &str) -> ParseResult<&str> {
    // Everything that doesn't start with known global option patterns
    // is considered remaining args for Pass 2
    take_while(|c| true)(input)
}
```

### 2. Modify `src/main.rs` for Two-Pass Flow

```rust
async fn main() -> Result<()> {
    let command_line = env::args().skip(1).collect::<Vec<_>>().join(" ");

    // ===== PASS 1: Parse global options only =====
    let (global_options, remaining_args) = match parse_global_options_only(&command_line) {
        Ok((remaining, global_opts)) => {
            let validated_globals = validate_global_options(&global_opts)?;
            (validated_globals, remaining)
        }
        Err(e) => return Err(e.into()),
    };

    // Handle help/version early (short-circuit)
    if global_options.help {
        show_help();
        return Ok(());
    }
    if global_options.version {
        show_version();
        return Ok(());
    }

    // Handle built-in commands (like "graph")
    if remaining_args.trim() == "graph" || remaining_args.trim().starts_with("graph ") {
        return handle_graph_command(&global_options, remaining_args).await;
    }

    // ===== LOAD CONFIG (using ottofile from pass 1) =====
    let config = load_config_with_ottofile(global_options.ottofile.as_ref())?;

    // ===== PASS 2: Parse tasks with config-aware disambiguation =====
    let mut parser = NomParser::new(Some(config))?;
    let tasks = parser.parse_tasks_with_config(remaining_args)?;

    // ===== EXECUTE =====
    execute_tasks(tasks, global_options).await
}
```

### 3. Update `src/cli/parser.rs` for Config-Aware Task Parsing

Add a new method to `NomParser`:

```rust
impl NomParser {
    /// Parse tasks with config-aware disambiguation (for Pass 2)
    pub fn parse_tasks_with_config(&mut self, input: &str) -> Result<Vec<ParsedTask>, ParseError> {
        let input = input.trim();

        // Handle empty input
        if input.is_empty() {
            return Ok(self.get_default_tasks());
        }

        // Parse task invocations with config-aware disambiguation
        let task_invocations = match parse_task_invocations_with_config(input, self.config.as_ref().unwrap()) {
            Ok((remaining, parsed)) => {
                if !remaining.trim().is_empty() {
                    return Err(ParseError::UnconsumedInput {
                        remaining: remaining.to_string(),
                    });
                }
                parsed
            }
            Err(nom::Err::Error(e)) | Err(nom::Err::Failure(e)) => return Err(e),
            Err(nom::Err::Incomplete(_)) => return Err(ParseError::IncompleteInput),
        };

        // Validate tasks against config
        let mut validated_tasks = Vec::new();
        for task_invocation in &task_invocations {
            if let Some(ref config) = self.config {
                let validated_task = validate_task_invocation(task_invocation, config)?;
                validated_tasks.push(validated_task);
            } else {
                return Err(ParseError::NoConfigFound {
                    searched_paths: vec!["otto.yml".to_string(), /* ... */],
                });
            }
        }

        Ok(validated_tasks)
    }
}
```

### 4. Create Config-Aware Task Parser in `src/cli/combinators.rs`

```rust
/// Parse task invocations with config-aware disambiguation
pub fn parse_task_invocations_with_config(
    input: &str,
    config: &ConfigSpec
) -> ParseResult<Vec<TaskInvocation>> {
    let known_tasks: HashSet<String> = config.tasks.keys().cloned().collect();

    context(
        "task invocations with config",
        all_consuming(
            separated_list0(
                whitespace1,
                task_invocation_with_config(&known_tasks, config)
            )
        )
    ).parse(input)
}

/// Parse a single task invocation with config-aware argument parsing
fn task_invocation_with_config(
    known_tasks: &HashSet<String>,
    config: &ConfigSpec
) -> impl Fn(&str) -> ParseResult<TaskInvocation> {
    move |input| {
        context("task invocation with config",
            map(
                pair(
                    known_task_name(known_tasks),
                    many0(preceded(
                        whitespace1,
                        task_argument_with_config(known_tasks, config)
                    ))
                ),
                |(name, arguments)| TaskInvocation {
                    name: name.to_string(),
                    arguments,
                }
            )
        ).parse(input)
    }
}

/// Parse task name that must be in the known tasks set
fn known_task_name(
    known_tasks: &HashSet<String>
) -> impl Fn(&str) -> ParseResult<&str> {
    move |input| {
        context("known task name",
            verify(identifier, |name: &str| known_tasks.contains(name))
        ).parse(input)
    }
}

/// Parse task argument with config-aware flag disambiguation
fn task_argument_with_config(
    known_tasks: &HashSet<String>,
    config: &ConfigSpec
) -> impl Fn(&str) -> ParseResult<TaskArgument> {
    move |input| {
        context("task argument with config",
            alt((
                task_argument_long_with_equals,  // --arg=value (unambiguous)
                task_argument_flag_or_value_with_config(known_tasks),
                task_argument_short_with_space,
            ))
        ).parse(input)
    }
}

/// The key function: disambiguate --flag vs --flag value using known task names
fn task_argument_flag_or_value_with_config(
    known_tasks: &HashSet<String>
) -> impl Fn(&str) -> ParseResult<TaskArgument> {
    move |input| {
        // Parse --flag first
        let (remaining, flag_name) = preceded(tag("--"), identifier).parse(input)?;

        // Look ahead to see what follows
        let (after_space, _) = whitespace1.parse(remaining)?;

        // Check if next token is a known task name
        if let Ok((_, next_token)) = identifier.parse(after_space) {
            if known_tasks.contains(next_token) {
                // Next token is a task name, so this --flag is boolean
                return Ok((remaining, TaskArgument {
                    name: flag_name.to_string(),
                    value: None,
                }));
            }
        }

        // Next token is not a task name, so --flag takes a value
        let (final_remaining, value) = preceded(whitespace1, argument_value).parse(remaining)?;
        Ok((final_remaining, TaskArgument {
            name: flag_name.to_string(),
            value: Some(value),
        }))
    }
}
```

## Benefits

### 1. **Eliminates Manual String Parsing**
- No more `split_whitespace()` and manual iteration
- Pure nom combinators throughout

### 2. **Solves Chicken-and-Egg Problem**
- Global options parsed first without config
- Config loaded using `--ottofile` from Pass 1
- Tasks parsed with full config validation

### 3. **Eliminates Grammar Ambiguity**
- **Task names become keywords** that segment the command line
- **Config-aware disambiguation**: `--flag taskname` vs `--flag value`
- **No more precedence rules**: Use semantic information instead of arbitrary ordering

### 4. **Clean Architecture**
- **Pass 1**: `global_options_parser.rs` - config-agnostic
- **Pass 2**: `parser.rs` - config-aware disambiguation
- **Separation of concerns**: Each pass has a single responsibility

### 5. **Proper nom Usage**
- Both passes use nom combinators
- Parameterized parsers for config-aware parsing
- Proper error handling with nom's error types
- Composable and extensible

### 6. **Maintainability**
- Easy to add new global options
- Easy to extend task parsing
- Clear code flow
- No more ambiguous grammar issues

## Example Flows

### Simple Task
```bash
# Command: "otto hello --greeting world"
# Pass 1: GlobalOptions::default(), remaining: "hello --greeting world"
# Pass 2: [ParsedTask { name: "hello", arguments: {"greeting": "world"} }]
```

### With Global Options
```bash
# Command: "otto --ottofile custom.yml --verbose hello --greeting world test --flag"
# Pass 1: GlobalOptions { ottofile: Some("custom.yml"), verbosity: Some(1) }, remaining: "hello --greeting world test --flag"
# Pass 2: [
#   ParsedTask { name: "hello", arguments: {"greeting": "world"} },
#   ParsedTask { name: "test", arguments: {"flag": true} }
# ]
```

### Help Short-Circuit
```bash
# Command: "otto --help"
# Pass 1: GlobalOptions { help: true }, remaining: ""
# Short-circuit: show_help() and exit
```

### Built-in Commands
```bash
# Command: "otto --ottofile custom.yml graph --format dot"
# Pass 1: GlobalOptions { ottofile: Some("custom.yml") }, remaining: "graph --format dot"
# Built-in handler: handle_graph_command() with parsed args
```

## Implementation Order

1. **Create `src/cli/global_options_parser.rs`** with nom combinators for global options only
2. **Add `parse_task_invocations_only()` to `src/cli/combinators.rs`**
3. **Add `parse_tasks_only()` method to `src/cli/parser.rs`**
4. **Modify `src/main.rs`** to use two-pass approach
5. **Test thoroughly** with existing examples
6. **Remove the disgusting `main_nom()` function** 🔥

## Testing Strategy

### Unit Tests
- Test Pass 1 parser with various global option combinations
- Test Pass 2 parser with various task combinations
- Test error handling for both passes

### Integration Tests
- Test full two-pass flow with existing examples
- Test help/version short-circuiting
- Test built-in command handling
- Test error propagation

### Regression Tests
- Ensure all existing functionality still works
- Verify error messages are still user-friendly
- Check that all examples still parse correctly

## Migration Notes

### Breaking Changes
- None expected - this is an internal refactor
- All existing command-line syntax should continue to work

### Backward Compatibility
- All existing examples should continue to work unchanged
- Error messages should remain user-friendly
- Performance should be equivalent or better

## Future Enhancements

Once this architecture is in place, future enhancements become easier:

1. **Add new global options** - just extend Pass 1 parser
2. **Add new task argument types** - just extend Pass 2 validation
3. **Add new built-in commands** - just extend built-in command handling
4. **Improve error messages** - each pass has its own error context
5. **Add shell completion** - can reuse both parsers for completion logic

## Config-Aware Disambiguation Example

Given an `otto.yml` with tasks: `build`, `test`, `deploy`

### Before (Ambiguous)
```bash
otto test --verbose build --output=dist
```

**Problem**: Is `--verbose` a boolean flag or does it take `build` as a value?

**Old approach**: Use precedence rules (arbitrary ordering in `alt()` combinator)

### After (Config-Aware)
```bash
otto test --verbose build --output=dist
```

**Solution**: Use known task names to disambiguate:

1. **Parse `test`** → known task ✓
2. **Parse `--verbose`** → check next token
3. **Next token is `build`** → known task ✓
4. **Therefore**: `--verbose` is a boolean flag, `build` starts new task
5. **Parse `build --output=dist`** → known task with argument

**Result**:
- Task 1: `test` with `--verbose` flag
- Task 2: `build` with `--output=dist` argument

### Comparison Example

```bash
otto test --verbose somevalue deploy --env=prod
```

**Config-aware parsing**:
1. `test` → known task ✓
2. `--verbose` → check next token
3. `somevalue` → NOT a known task ✗
4. **Therefore**: `--verbose` takes `somevalue` as value
5. `deploy` → known task ✓

**Result**:
- Task 1: `test` with `--verbose=somevalue`
- Task 2: `deploy` with `--env=prod`

## Conclusion

This config-aware two-pass approach eliminates the grammar ambiguity by using semantic information (known task names) to make parsing decisions. It maintains all existing functionality while providing a clean, maintainable, and extensible foundation that properly leverages nom's parser combinator capabilities without artificial precedence rules.
