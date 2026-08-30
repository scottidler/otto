# Makefile to Otto Converter - Design & Architecture

> **Status note (2026-08-30):** this is the pre-implementation design for
> `otto Convert` (archived history, kept for the architecture rationale).
> The feature shipped; the builtin's name is capitalized (`Convert`, not
> `convert` - see `src/app.rs`'s `Builtin` enum), and it now carries a real
> `Diagnostic` warning system (`src/makefile/diagnostic.rs`) that this
> original draft only sketched. The four places below where the draft's
> description diverged from what shipped are corrected inline; nothing else
> in this doc was re-verified against the current `src/makefile/` module.

## Overview

This feature enables users to convert existing Makefiles to Otto YAML format via stdin/stdout. This allows seamless migration from Make-based workflows to Otto.

**Command Usage:**
```bash
cat Makefile | otto Convert > otto.yml
# or
otto Convert < Makefile > otto.yml
```

## Feature Requirements

### Core Functionality
1. Accept Makefile content via stdin
2. Parse Makefile syntax into structured representation
3. Convert Makefile constructs to Otto YAML equivalents
4. Output formatted Otto YAML to stdout
5. Handle common Makefile patterns and idioms
6. Provide meaningful error messages for unsupported constructs (**shipped**: every construct the converter cannot represent faithfully becomes a `Diagnostic` printed to stderr, naming the source line; `--strict` turns a non-empty diagnostic list into a non-zero exit instead of a silent conversion. See `src/makefile/diagnostic.rs`.)

### Supported Makefile Features (Phase 1)
- Variable definitions (`VAR := value`, `VAR = value`, `VAR ?= value`)
- Shell command variables (`VAR := $(shell cmd)`)
- `.PHONY` declarations
- `.DEFAULT_GOAL` specification
- Target definitions with dependencies
- Multi-line commands (using backslash continuation)
- Comments (convert to help text where applicable)
- Target-specific comments (becomes task help)

### Unsupported/Deferred Features
- Pattern rules (`%.o: %.c`)
- Conditional directives (`ifeq`, `ifdef`, etc.)
- Include directives
- Automatic variables (`$@`, `$<`, `$^`, etc.) - will warn user (**shipped**: `converter.rs` emits `` automatic variable `$@` has no otto equivalent; left as written `` and leaves the reference untouched rather than mangling it)
- Complex variable expansion with functions
- Recursive make invocations

## Architecture

### Module Structure

```
src/
  cli/
    commands/
      convert.rs        # New: Convert command implementation
      mod.rs            # Add ConvertCommand export
    mod.rs              # Export ConvertCommand
  makefile/             # New module for Makefile parsing
    mod.rs              # Module exports
    parser.rs           # Makefile parser
    ast.rs              # Abstract syntax tree for Makefile
    converter.rs        # AST to Otto conversion logic
  main.rs               # Add convert command handling
```

### Data Flow

```
stdin (Makefile)
    ↓
ConvertCommand::execute()
    ↓
MakefileParser::parse()
    ↓
MakefileAst (intermediate representation)
    ↓
OttoConverter::convert()
    ↓
OttoConfig (Otto data structures)
    ↓
serde_yaml::to_string()
    ↓
stdout (YAML)
```

## Component Design

### 1. MakefileAst (Abstract Syntax Tree)

**Purpose:** Intermediate representation of parsed Makefile

```rust
// src/makefile/ast.rs

pub struct MakefileAst {
    pub variables: Vec<Variable>,
    pub default_goal: Option<String>,
    pub phony_targets: HashSet<String>,
    pub targets: Vec<Target>,
}

pub struct Variable {
    pub name: String,
    pub value: String,
    pub assignment_type: AssignmentType,
}

pub enum AssignmentType {
    Simple,          // :=
    Recursive,       // =
    Conditional,     // ?=
    Append,          // +=
    ShellExecution,  // $(shell ...)
}

pub struct Target {
    pub name: String,
    pub dependencies: Vec<String>,
    pub commands: Vec<String>,
    pub comment: Option<String>,  // Comment immediately preceding target
    pub is_phony: bool,
}
```

### 2. MakefileParser

**Purpose:** Parse raw Makefile text into AST

```rust
// src/makefile/parser.rs

pub struct MakefileParser {
    content: String,
    line_number: usize,
}

impl MakefileParser {
    pub fn new(content: String) -> Self;
    pub fn parse(&mut self) -> Result<MakefileAst>;

    // Private parsing methods
    fn parse_variable(&mut self, line: &str) -> Result<Option<Variable>>;
    fn parse_target(&mut self, lines: &[String], index: &mut usize) -> Result<Option<Target>>;
    fn parse_dependencies(&self, dep_line: &str) -> Vec<String>;
    fn parse_commands(&self, lines: &[String], index: &mut usize) -> Vec<String>;
    fn is_phony_declaration(&self, line: &str) -> Option<Vec<String>>;
    fn extract_default_goal(&self, line: &str) -> Option<String>;
    fn handle_line_continuation(&self, lines: &[String], index: &mut usize) -> String;
}
```

**Key Parsing Logic:**

- Line-by-line parsing with state machine
- Detect variable assignments (check for `:=`, `?=`, `+=`, `=`, longest-operator-first so `?=`/`+=` are never split by an eager bare-`=` match - this draft originally listed `=` before `?=`/`+=`, backwards from what `parser.rs`'s `assignment_ops` actually needs and ships)
- Detect target definitions (lines ending with `:`)
- Handle tab-indented commands under targets
- Track `.PHONY` and `.DEFAULT_GOAL` directives
- Capture preceding comments for help text
- Handle backslash line continuations

### 3. OttoConverter

**Purpose:** Convert MakefileAst to Otto configuration structures

```rust
// src/makefile/converter.rs

pub struct OttoConverter {
    ast: MakefileAst,
}

impl OttoConverter {
    pub fn new(ast: MakefileAst) -> Self;
    pub fn convert(&self) -> Result<ConfigSpec>;

    // Private conversion methods
    fn convert_variables(&self) -> HashMap<String, String>;
    fn convert_targets(&self) -> Result<TaskSpecs>;
    fn convert_target_to_task(&self, target: &Target) -> Result<TaskSpec>;
    fn detect_shell_variables(&self, value: &str) -> Option<String>;
    fn generate_help_text(&self, target: &Target) -> Option<String>;
    fn determine_default_tasks(&self) -> Vec<String>;
}
```

**Conversion Rules:**

1. **Variables → `otto.envs`**
   - Simple/Recursive assignments: direct mapping
   - Conditional assignments: direct mapping (user-provided env vars override)
   - Shell executions: preserve `$(...)` syntax (Otto will evaluate)

2. **Targets → `tasks`**
   - Target name becomes task name
   - Dependencies become `before` field
   - Commands become `bash:` field with proper shebang
   - Comments become `help:` field
   - `.PHONY` targets remain as-is (Otto doesn't distinguish)

3. **`.DEFAULT_GOAL` → `otto.tasks`**
   - Default goal becomes the default task list

4. **Commands Processing:**
   - Combine multi-line commands
   - Preserve command structure
   - Add `#!/bin/bash` shebang if not present
   - Handle command prefixes (`@`, `-`)

### 4. ConvertCommand

**Purpose:** CLI command that orchestrates the conversion

```rust
// src/cli/commands/convert.rs

use clap::Parser;
use std::io::{self, Read, Write};

#[derive(Parser, Debug)]
#[command(name = "convert")]
#[command(about = "Convert Makefile to Otto YAML format")]
pub struct ConvertCommand {
    /// Treat warnings as errors
    #[arg(long)]
    strict: bool,

    /// Output file (default: stdout)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

impl ConvertCommand {
    pub fn execute(&self) -> Result<()> {
        // Read from stdin
        let mut content = String::new();
        io::stdin().read_to_string(&mut content)?;

        // Parse Makefile
        let mut parser = MakefileParser::new(content);
        let ast = parser.parse()?;

        // Convert to Otto
        let converter = OttoConverter::new(ast);
        let config = converter.convert()?;

        // Serialize to YAML
        let yaml = serde_yaml::to_string(&config)?;

        // Write to stdout or file
        if let Some(output_path) = &self.output {
            std::fs::write(output_path, yaml)?;
        } else {
            io::stdout().write_all(yaml.as_bytes())?;
        }

        Ok(())
    }
}
```

## Integration Steps

### Step 1: Create Makefile AST Module
- Create `src/makefile/mod.rs`
- Create `src/makefile/ast.rs` with data structures
- Add to `src/lib.rs` module exports

### Step 2: Implement Makefile Parser
- Create `src/makefile/parser.rs`
- Implement line-by-line parsing logic
- Handle all supported Makefile constructs
- Write unit tests for parser

### Step 3: Implement Otto Converter
- Create `src/makefile/converter.rs`
- Implement AST to Otto conversion logic
- Handle edge cases and special patterns
- Write unit tests for converter

### Step 4: Create Convert Command
- Create `src/cli/commands/convert.rs`
- Implement stdin/stdout handling
- Add command to CLI router in `main.rs`
- Export from `src/cli/commands/mod.rs`

### Step 5: Add Command Routing
- Update `src/main.rs` to handle "convert" command
- Follow pattern of existing commands (clean, history, stats)
- Add early exit for convert command

### Step 6: Create Test Infrastructure
- Create `tests/makefile_converter/` directory
- Add example Makefiles for testing
- Create expected Otto YAML outputs
- Implement integration tests

### Step 7: Integration Tests
- Test with existing Makefiles in `examples/`
- Test with real-world Makefiles from third-party projects
- Verify conversion correctness
- Test error handling
- Test edge cases
- Document any unsupported patterns discovered

## Testing Strategy

> **What actually shipped (2026-08-30), replacing this section's original
> plan wholesale:** the directory structure, per-fixture `.yml` files, and
> `test_tatari_tv_makefiles` sketched below were never built; `tests/fixtures/`
> does not exist. The real suite is `tests/makefile_converter_test.rs`,
> against fixtures at `makefiles/<name>/Makefile` (repo-root, not under
> `tests/`), each with a committed `expected.yml` compared as a `ConfigSpec`
> rather than as text, plus the exact `Diagnostic` list the fixture must
> produce. Fixtures cover the negative cases this draft's original assertions
> missed entirely (`$(shell ...)`, `$(VAR)`, pattern rules, multi-target
> lines) - see the test file's own module doc for the current fixture list.

## Error Handling

### Parser Errors
- Malformed target definitions
- Invalid variable assignments
- Unclosed line continuations
- Invalid `.PHONY` or `.DEFAULT_GOAL` syntax

### Conversion Warnings
- Unsupported features detected (pattern rules, conditionals)
- Automatic variables that need manual replacement
- Complex variable expansions that may not work

### Example Error Output
```
Error parsing Makefile at line 23:
  Unexpected indentation: commands must be tab-indented

Warning: Pattern rule detected at line 15:
  %.o: %.c
  Pattern rules are not supported. Please convert manually.

Warning: Automatic variable $@ detected in target 'build':
  These must be replaced manually in Otto.
```

## Implementation Steps

1. **Create test fixtures** - Set up test directory with example Makefiles and expected outputs
2. **Implement AST structures** - Define data structures for Makefile representation
3. **Implement parser (TDD)** - Parse Makefiles into AST, test each feature
4. **Implement converter (TDD)** - Convert AST to Otto structures, test each feature
5. **Create convert command** - Wire up CLI command
6. **Add command routing** - Integrate into main.rs
7. **Integration testing** - Test with real Makefiles from examples/
8. **Code quality checks** - Run cargo fmt, clippy, and all tests
9. **Run otto all** - Final validation before completion

## Success Criteria

- All unit tests pass
- All integration tests pass
- Converter successfully processes all example Makefiles
- No clippy warnings
- Code formatted with cargo fmt
- No dead code (or properly prefixed with `_`)
- `cargo test` passes with no errors or warnings
- `otto all` completes successfully

## Future Enhancements (Out of Scope)

- Support for pattern rules via templates
- Conditional directive handling
- Automatic variable translation hints
- Interactive conversion mode with user prompts
- Bidirectional conversion (Otto → Makefile)
- Make compatibility mode in Otto
