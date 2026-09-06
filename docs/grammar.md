# Otto CLI Grammar Specification

This document defines the formal grammar for the Otto CLI command-line interface using multiple notation systems familiar to programming language experts.

## Table of Contents

1. [Overview](#overview)
2. [EBNF Grammar](#ebnf-grammar)
3. [BNF Grammar](#bnf-grammar)
4. [Lexical Analysis](#lexical-analysis)
5. [Semantic Rules](#semantic-rules)
6. [Grammar Ambiguities](#grammar-ambiguities)
7. [Examples](#examples)

## Overview

Otto CLI uses a two-pass parsing approach:
- **Pass 1**: Extract global options that can appear anywhere in the command line
- **Pass 2**: Parse task invocations with their arguments, validated against configuration

The grammar supports:
- Global options (affecting Otto behavior)
- Built-in commands (`Graph`, `History`, `Stats`, `Clean`, `Convert`, `Upgrade`, plus `help`)
- Task invocations with typed arguments
- Mixed global and task-specific options

## EBNF Grammar

Extended Backus-Naur Form (ISO/IEC 14977):

```ebnf
(* Otto CLI Grammar *)

command_line = [ global_options ], [ command | task_invocations ] ;

global_options = { global_option } ;

global_option = global_option_long_equals
              | global_option_long_space
              | global_option_short_space
              | global_option_flag ;

global_option_long_equals = "--", identifier, "=", argument_value ;
global_option_long_space  = "--", identifier, whitespace1, argument_value ;
global_option_short_space = "-", short_char, whitespace1, argument_value ;
global_option_flag       = ( "--", identifier ) | ( "-", short_char ) ;

command = builtin_command, { whitespace1, task_argument }
        | "help", [ task_name ] ;

builtin_command = "Clean" | "Convert" | "Graph" | "History" | "Stats" | "Upgrade" ;

graph_options = { graph_option } ;
graph_option = "--format", whitespace1, graph_format
             | "--output", whitespace1, file_path ;

graph_format = "ascii" | "dot" | "svg" | "png" | "pdf" ;

task_invocations = task_invocation, { whitespace1, task_invocation } ;

task_invocation = task_name, { whitespace1, task_argument } ;

task_argument = task_argument_long_equals
              | task_argument_long_space
              | task_argument_short_space
              | task_argument_flag ;

task_argument_long_equals = "--", identifier, "=", argument_value ;
task_argument_long_space  = "--", identifier, whitespace1, argument_value ;
task_argument_short_space = "-", letter, whitespace1, argument_value ;
task_argument_flag       = ( "--", identifier ) | ( "-", letter ) ;

(* Lexical Rules *)

identifier = ( letter | "_" ), { letter | digit | "_" | "-" } ;
task_name = identifier ;
argument_value = quoted_string | unquoted_value ;
quoted_string = '"', { ? any character except '"' ? }, '"'
              | "'", { ? any character except "'" ? }, "'" ;
unquoted_value = { ? any non-whitespace character ? } ;

short_char = "C" | "o" | "j" | "t" | "h" | "V" ;
letter = "a" | "b" | "c" | ? ... ? | "z" | "A" | "B" | ? ... ? | "Z" ;
digit = "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" ;
whitespace1 = { " " | "\t" | "\n" | "\r" } ;
file_path = argument_value ;
```

## BNF Grammar

Classic Backus-Naur Form:

```bnf
<command_line> ::= <global_options> <command>
                 | <global_options> <task_invocations>
                 | <global_options>
                 | <command>
                 | <task_invocations>
                 | <empty>

<global_options> ::= <global_option>
                   | <global_options> <global_option>
                   | <empty>

<global_option> ::= <global_option_long_equals>
                  | <global_option_long_space>
                  | <global_option_short_space>
                  | <global_option_flag>

<global_option_long_equals> ::= "--" <global_option_name> "=" <argument_value>
<global_option_long_space>  ::= "--" <global_option_name> <whitespace1> <argument_value>
<global_option_short_space> ::= "-" <global_short_char> <whitespace1> <argument_value>
<global_option_flag>       ::= "--" <global_flag_name>
                             | "-" <global_short_flag>

<global_option_name> ::= "cwd" | "ottofile" | "jobs" | "format" | "log-level"
<global_flag_name>   ::= "help" | "version" | "tui" | "tasks" | "list-subtasks" | "no-prefix"
<global_short_char>  ::= "C" | "o" | "j"
<global_short_flag>  ::= "h" | "V" | "t"

<command> ::= <builtin_command>
            | <help_command>

<builtin_command> ::= <graph_command>
                    | "Clean" | "Convert" | "History" | "Stats" | "Upgrade"

<graph_command>   ::= "Graph" <graph_options>
<help_command>    ::= "help" <task_name>
                  | "help"

<graph_options> ::= <graph_option>
                  | <graph_options> <graph_option>
                  | <empty>

<graph_option> ::= "--format" <whitespace1> <graph_format>
                 | "--output" <whitespace1> <file_path>

<graph_format> ::= "ascii" | "dot" | "svg" | "png" | "pdf"

<task_invocations> ::= <task_invocation>
                     | <task_invocations> <whitespace1> <task_invocation>

<task_invocation> ::= <task_name> <task_arguments>

<task_arguments> ::= <task_argument>
                   | <task_arguments> <whitespace1> <task_argument>
                   | <empty>

<task_argument> ::= <task_argument_long_equals>
                  | <task_argument_long_space>
                  | <task_argument_short_space>
                  | <task_argument_flag>

<task_argument_long_equals> ::= "--" <identifier> "=" <argument_value>
<task_argument_long_space>  ::= "--" <identifier> <whitespace1> <argument_value>
<task_argument_short_space> ::= "-" <letter> <whitespace1> <argument_value>
<task_argument_flag>       ::= "--" <identifier>
                             | "-" <letter>

<task_name>      ::= <identifier>
<identifier>     ::= <id_start> <id_continue>
<id_start>       ::= <letter> | "_"
<id_continue>    ::= <id_char>
                   | <id_continue> <id_char>
                   | <empty>
<id_char>        ::= <letter> | <digit> | "_" | "-"

<argument_value> ::= <quoted_string> | <unquoted_value>
<quoted_string>  ::= '"' <string_content> '"'
                   | "'" <string_content> "'"
<string_content> ::= <string_char>
                   | <string_content> <string_char>
                   | <empty>
<string_char>    ::= <any_char_except_quote>
<unquoted_value> ::= <nonws_char>
                   | <unquoted_value> <nonws_char>

<letter>         ::= "a" | "b" | ... | "z" | "A" | "B" | ... | "Z"
<digit>          ::= "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"
<whitespace1>    ::= <ws_char>
                   | <whitespace1> <ws_char>
<ws_char>        ::= " " | "\t" | "\n" | "\r"
<nonws_char>     ::= <any_char_except_whitespace>
<file_path>      ::= <argument_value>
<empty>          ::= ""
```

## Lexical Analysis

### Token Types

```rust
// Terminal symbols (tokens)
enum Token {
    // Literals
    Identifier(String),
    QuotedString(String),
    UnquotedValue(String),

    // Keywords
    Graph,
    Help,
    Version,

    // Operators
    DoubleDash,      // "--"
    SingleDash,      // "-"
    Equals,          // "="

    // Whitespace
    Whitespace,

    // Special
    EOF,
}
```

### Lexical Rules

```
IDENTIFIER     ::= [a-zA-Z_][a-zA-Z0-9_-]*
QUOTED_STRING  ::= "([^"]*)" | '([^']*)'
UNQUOTED_VALUE ::= [^\s]+
WHITESPACE     ::= [\s]+
DOUBLE_DASH    ::= "--"
SINGLE_DASH    ::= "-"
EQUALS         ::= "="
```

### Tokenization Order

1. **Whitespace** (consumed, not returned)
2. **Keywords** (the capitalized builtins, plus `help`)
3. **Operators** (`--`, `-`, `=`)
4. **Quoted strings** (higher precedence than unquoted)
5. **Identifiers** (alphanumeric + underscore + dash)
6. **Unquoted values** (fallback for non-whitespace)

## Semantic Rules

### Global Options

This table is the whole global surface: every row below appears in
`otto --help`, and nothing in `otto --help` is missing from it. Re-derived
against the binary on 2026-08-30, which is when `--api`, `--home`/`-H`,
`--verbosity`/`-v` and a bare `--verbose` were struck: none of them existed.
`otto.home` and `otto.verbosity` were deleted from the ottofile schema the same
week (see `docs/commands/ottofile-strict-schema-migration.md`), so this page no
longer advertises either spelling.

| Option | Short | Type | Description |
|--------|-------|------|-------------|
| `--cwd` | `-C` | `DIR` | Change to DIR before doing anything |
| `--ottofile` | `-o` | `PATH` | Path to the ottofile (env: `OTTOFILE`) |
| `--list-subtasks` | | `Flag` | List all foreach subtasks and exit |
| `--tasks` | | `Flag` | Print the machine-readable task list and exit |
| `--format` | | `yaml\|json` | Output format for `--tasks`; yaml on a tty, json when piped |
| `--jobs` | `-j` | `N` | Number of parallel jobs |
| `--tui` | `-t` | `Flag` | Enable the interactive TUI dashboard |
| `--no-prefix` | | `Flag` | Suppress the `[task]` prefix on task output |
| `--log-level` | | `LEVEL` | Verbosity of otto's own log file (`off`..`trace`) |
| `--help` | `-h` | `Flag` | Show help message |
| `--version` | `-V` | `Flag` | Show version |

Otto's own state directory is `$OTTO_HOME` (or `$HOME/.otto`), an environment
variable with no flag spelling; the database under it can be moved on its own
with `$OTTO_DB_PATH`.

### Per-Task Option: `--Serial`

Not a global option — it is auto-injected onto every `foreach` task's own
argument parser (`BUILTIN_PARAMS` in `src/cli/builtins.rs`), so it only
appears after a foreach task's name, e.g. `otto logs --Serial`.

| Option | Short | Type | Description |
|--------|-------|------|-------------|
| `--Serial` | | `Flag` | Force one item at a time for this foreach task's run, overriding `foreach.parallel: true` |

Rejected, at run setup rather than at load time, when combined with
`foreach.jobs` on the same task: `Task '<name>': --Serial cannot be combined
with foreach.jobs` (`docs/commands/buffered-foreach.md` has the full rationale
— `jobs` only makes sense when items run concurrently).

### Task Arguments

Task arguments are dynamically typed based on configuration:

```rust
enum ValidatedValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Path(PathBuf),
    Url(String),
}
```

### Parameter Types

```rust
enum ParamType {
    FLG,  // Flag (boolean, no value)
    OPT,  // Optional (requires value)
    POS,  // Positional (requires value)
}
```

### Validation Rules

1. **Global options** are validated against a fixed schema
2. **Task arguments** are validated against dynamic configuration
3. **Short flags** are mapped to long names via configuration
4. **Type coercion** follows parameter specifications
5. **Default values** are applied for missing optional parameters
6. **Required parameters** must be provided or have defaults

## Grammar Ambiguities

### Inherent Ambiguities

The grammar contains intentional ambiguities that are resolved through precedence:

#### 1. Global vs Task Arguments

```bash
otto --jobs 4 build
```

**Ambiguous interpretations:**
- `--jobs` is a global option with value `4`, `build` is a task
- `--jobs` is a task argument for task `4` with value `build`

**Resolution:** Global options are parsed first (Pass 1), remaining tokens go to task parsing (Pass 2).

#### 2. Flag vs Argument with Space

```bash
otto --no-prefix build
```

**Ambiguous interpretations:**
- `--no-prefix` is a flag, `build` is a task name
- `--no-prefix` is an argument with value `build`

**Resolution:** Known global flags are recognized by name, unknown flags are treated as arguments with values.

#### 3. Task Argument Order

```bash
otto build --flag value
```

**Ambiguous interpretations:**
- `--flag` is a boolean flag, `value` is a positional argument
- `--flag` is an argument with value `value`

**Resolution:** Arguments with values are parsed before flags in the `alt()` combinator.

### Disambiguation Strategy

```rust
// Parser precedence (highest to lowest)
task_argument = alt((
    task_argument_long_with_equals,  // --arg=value (highest)
    task_argument_long_with_space,   // --arg value
    task_argument_short_with_space,  // -a value
    task_argument_flag,              // --flag (lowest)
))
```

## Examples

### Basic Task Invocation

```bash
otto build
```

**Parse tree:**
```
command_line
├── global_options: []
└── task_invocations
    └── task_invocation
        ├── task_name: "build"
        └── task_arguments: []
```

### Global Options with Task

```bash
otto --ottofile custom.yml --jobs 4 build --verbose
```

**Parse tree:**
```
command_line
├── global_options
│   ├── global_option: ottofile="custom.yml"
│   └── global_option: jobs=4
└── task_invocations
    └── task_invocation
        ├── task_name: "build"
        └── task_arguments
            └── task_argument: verbose=flag
```

### Multiple Tasks with Arguments

```bash
otto test --coverage build --release deploy --env production
```

**Parse tree:**
```
command_line
├── global_options: []
└── task_invocations
    ├── task_invocation
    │   ├── task_name: "test"
    │   └── task_arguments
    │       └── task_argument: coverage=flag
    ├── task_invocation
    │   ├── task_name: "build"
    │   └── task_arguments
    │       └── task_argument: release=flag
    └── task_invocation
        ├── task_name: "deploy"
        └── task_arguments
            └── task_argument: env="production"
```

### Graph Command

```bash
otto Graph --format dot --output graph.dot
```

**Parse tree:**
```
command_line
├── global_options: []
└── command
    └── graph_command
        ├── keyword: "Graph"
        └── graph_options
            ├── graph_option: format="dot"
            └── graph_option: output="graph.dot"
```

### Complex Mixed Example

```bash
otto --ottofile=build.yml --no-prefix test --unit --integration build --release=true
```

**Parse tree:**
```
command_line
├── global_options
│   ├── global_option: ottofile="build.yml"
│   └── global_option: no-prefix=flag
└── task_invocations
    ├── task_invocation
    │   ├── task_name: "test"
    │   └── task_arguments
    │       ├── task_argument: unit=flag
    │       └── task_argument: integration=flag
    └── task_invocation
        ├── task_name: "build"
        └── task_arguments
            └── task_argument: release="true"
```

## Implementation Notes

The real implementation is `src/cli/parser.rs`. There is no separate two-pass
parser built on `nom`, no `enum ParseError`, and no `enum ValidatedValue` —
`nom` is not a dependency of this crate. The actual split between global
options and task invocations is `partitions()`: it finds where each task
invocation starts (via `indices()`, matching against the loaded config's task
names) and slices the argument vector at those boundaries, one slice per task
invocation plus a leading slice of otto's own global options. Each slice is
then handed to that task's own `clap`-derived parser. `contains_flag()`
(`parser.rs`) is the one place a flag like `--Serial` is looked up directly in
an argument slice, and it skips over any option's value so a flag spelled
inside a value's text (`--msg --Serial`, the string) doesn't false-match.

This grammar specification is a formal description of the CLI surface, not of
the parser's internal architecture.
