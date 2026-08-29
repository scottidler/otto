# Otto Task Flag Support

This document describes how Otto's task system handles different types of command-line flags and how to configure them in YAML.

## Overview

Otto supports two types of flags in task definitions:

1. **Boolean flags** (without values) - represent a boolean value that is either present or absent
2. **Argument-style flags** (with values) - where the user passes a specific value

The system automatically determines the flag type based on the YAML configuration and handles the command-line parsing accordingly.

## How Flag Types Are Determined

Otto uses an automatic detection system based on the parameter configuration:

```rust
enum ParamType {
    FLG,  // Flag (boolean, no value)
    OPT,  // Optional (requires value)
    POS,  // Positional (requires value)
}
```

The detection logic works as follows:

1. If the parameter has short (`-x`) or long (`--flag`) form AND the `default` value is `"true"` or `"false"`, it becomes a **boolean flag** (`ParamType::FLG`)
2. If the parameter has no short/long form, it becomes a **positional argument** (`ParamType::POS`)
3. Otherwise, it defaults to an **optional argument** (`ParamType::OPT`) that requires a value

## YAML Configuration Examples

### Boolean Flags (No Values)

Boolean flags are created by setting the `default` to `"true"` or `"false"`:

```yaml
tasks:
  build:
    params:
      -v|--verbose:
        default: false
        help: Enable verbose output
      -f|--force:
        default: false
        help: Force rebuild
      --dry-run:
        default: false
        help: Show what would be done without executing
    bash: |
      echo "Verbose: ${verbose}"
      echo "Force: ${force}"
      echo "Dry run: ${dry_run}"
```

**Command-line usage:**
```bash
# Set flags to true by including them
otto build --verbose --force --dry-run
otto build -v -f --dry-run

# Flags default to false when not specified
otto build  # verbose=false, force=false, dry_run=false
```

### Argument-Style Flags (With Values)

For flags that accept values, configure them without boolean defaults:

```yaml
tasks:
  deploy:
    params:
      -e|--env:
        default: development
        choices: [development, staging, production]
        help: Target environment
      -c|--config:
        help: Path to config file
      --timeout:
        default: 30
        help: Timeout in seconds
      --port:
        default: 8080
        help: Server port number
    bash: |
      echo "Environment: ${env}"
      echo "Config: ${config:-'not provided'}"
      echo "Timeout: ${timeout}"
      echo "Port: ${port}"
```

**Command-line usage:**
```bash
# Provide values for flags
otto deploy --env production --config /path/to/config.yml --timeout 60
otto deploy -e staging -c config.yml --port 3000

# Use defaults when not specified
otto deploy  # env=development, timeout=30, port=8080
```

### Mixed Configuration

You can combine both types in the same task:

```yaml
tasks:
  test:
    params:
      # Boolean flags
      -v|--verbose:
        default: false
        help: Enable verbose output
      --coverage:
        default: false
        help: Generate coverage report
      --watch:
        default: false
        help: Watch for file changes

      # Argument flags
      -p|--pattern:
        default: "**/*.test.js"
        help: Test file pattern
      --reporter:
        choices: [spec, json, junit, tap]
        default: spec
        help: Test reporter format
      --timeout:
        default: 5000
        help: Test timeout in milliseconds

    bash: |
      echo "=== Test Configuration ==="
      echo "Verbose: ${verbose}"
      echo "Coverage: ${coverage}"
      echo "Watch: ${watch}"
      echo "Pattern: ${pattern}"
      echo "Reporter: ${reporter}"
      echo "Timeout: ${timeout}ms"

      # Use the configuration in your test command
      if [ "${coverage}" = "true" ]; then
        echo "Running with coverage..."
      fi
```

**Command-line usage:**
```bash
# Boolean flags only
otto test --verbose --coverage --watch

# Argument flags only
otto test --pattern "src/**/*.test.js" --reporter json --timeout 10000

# Mixed usage
otto test -v --coverage -p "*.spec.js" --reporter junit --timeout 8000

# Use short forms where available
otto test -v -p "test/**/*.js"
```

## Advanced Configuration Features

### Choices and Validation

Argument-style flags support validation through the `choices` field:

```yaml
tasks:
  package:
    params:
      --format:
        choices: [tar, zip, deb, rpm]
        default: tar
        help: Package format
      --compression:
        choices: [gzip, bzip2, xz, none]
        default: gzip
        help: Compression method
    bash: |
      echo "Creating ${format} package with ${compression} compression"
```

### Dynamic Choices from a Command

When the valid set isn't knowable when the ottofile is written (a service
registry, a set of AWS profiles, whatever the environment currently holds), use
`choices-command:` instead of `choices:`. Otto runs the command and treats its
non-empty stdout lines, whitespace-trimmed, as the allowed values:

```yaml
tasks:
  switch:
    params:
      -s|--svc:
        choices-command: "scripts/list-services.sh"
        help: Service to switch to
    bash: |
      echo "switching to ${svc}"
```

`choices` and `choices-command` are mutually exclusive; declaring both is a
config error naming the param.

**When the command runs.** Only when a value actually has to be validated: you
passed arguments to the task, or a value propagated into the task from a
dependent (a dependency's `choices-command` runs when it receives a propagated
value, because its param has to validate too). It runs at most once per otto
invocation, no matter how many places ask.

**When it never runs.** Every help surface (`otto --help`, `otto help <task>`,
`<task> --help`) and `otto --tasks`. Help renders `[dynamic choices: <command>]`
in place of the value list, and `--tasks` reports the command in a
`choices-command` field: both name the command so you can run it yourself. This
is the same rule `foreach: command:` follows.

**Execution context**, identical to `foreach: command:`: `sh -c`, cwd = the
ottofile's directory, environment = the inherited environment plus the resolved
global `envs:`. Task params are not available.

**Failure is loud.** A non-zero exit is an error naming the task, the param, the
command, and the exit code. So is *empty* output: unlike a `foreach:` that
legitimately matches nothing, a param whose valid set is empty could never be
given a value, so that is a misconfiguration rather than a valid state.

A nested otto that would resolve the same `task:param` errors instead of
recursing (the guard rides `OTTO_CHOICES_COMMAND`).

### Help Text and Metadata

All flag types support comprehensive help documentation:

```yaml
tasks:
  serve:
    help: Start the development server
    params:
      -p|--port:
        default: 3000
        help: Port number for the server
        metavar: PORT
      --host:
        default: localhost
        help: Host address to bind to
        metavar: ADDRESS
      -d|--daemon:
        default: false
        help: Run server in background
      --log-level:
        choices: [debug, info, warn, error]
        default: info
        help: Set logging verbosity
    bash: |
      echo "Starting server on ${host}:${port}"
      echo "Log level: ${log_level}"
      if [ "${daemon}" = "true" ]; then
        echo "Running in daemon mode"
      fi
```

## Environment Variable Integration

All parameters (both boolean flags and argument flags) are automatically available as environment variables in the task script:

- Flag names are converted to environment variable names
- Dashes in long flag names become underscores (e.g., `--dry-run` → `${dry_run}`)
- Boolean flags have values `"true"` or `"false"` as strings

## Flag Syntax Support

Otto supports multiple flag syntax patterns:

### Boolean Flags
```bash
# Long form
otto task --verbose --force

# Short form
otto task -v -f

# Combined short forms
otto task -vf
```

### Argument Flags
```bash
# Long form with equals
otto task --env=production --timeout=30

# Long form with space
otto task --env production --timeout 30

# Short form with space
otto task -e production -t 30

# Mixed syntax
otto task --env=production -t 30 --verbose
```

## Best Practices

### 1. Consistent Naming
Use consistent naming patterns for similar functionality across tasks:

```yaml
# Good: Consistent verbose flag
tasks:
  build:
    params:
      -v|--verbose:
        default: false
        help: Enable verbose output

  test:
    params:
      -v|--verbose:
        default: false
        help: Enable verbose output
```

### 2. Meaningful Defaults
Provide sensible defaults that work for the most common use cases:

```yaml
tasks:
  deploy:
    params:
      --env:
        default: development  # Safe default
        choices: [development, staging, production]
      --timeout:
        default: 300  # Reasonable timeout
```

### 3. Clear Help Text
Write descriptive help text that explains the flag's purpose and any constraints:

```yaml
tasks:
  backup:
    params:
      --retention:
        default: 30
        help: Number of days to retain backups (1-365)
      --compress:
        default: true
        help: Compress backup files to save space
```

### 4. Validation with Choices
Use the `choices` field to prevent invalid values:

```yaml
tasks:
  build:
    params:
      --target:
        choices: [debug, release, profile]
        default: debug
        help: Build target configuration
```

## Implementation Notes

### Current Limitation
There is an incomplete implementation in the `param_to_arg()` function in `src/cli/parser.rs`. The function doesn't properly handle the `ParamType::FLG` case to set `action(clap::ArgAction::SetTrue)` for boolean flags. This may affect boolean flag parsing and should be addressed.

### Internal Processing
1. Parameters are parsed from YAML using the `deserialize_param_map` function
2. Flag types are automatically determined based on the presence of boolean defaults
3. Command-line arguments are processed using a two-pass parser (global options first, then task arguments)
4. Parameter values are made available both as task values and environment variables

## Examples from the Codebase

The Otto repository contains several examples demonstrating flag usage:

- `examples/old/ex1/otto.yml` - Shows boolean flag configuration with `default: false`
- `examples/ex1/otto.yml` - Demonstrates argument-style flags with choices and defaults
- `examples/ex2/.otto.yml` - Examples of metavar and dest field usage

## Summary

Otto's flag system provides flexible support for both boolean and argument-style flags through automatic type detection based on YAML configuration. The key is using `default: "true"` or `default: "false"` for boolean flags, while other configurations default to argument-style flags that require values.
