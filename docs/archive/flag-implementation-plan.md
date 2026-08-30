# Otto Flag Implementation Plan

## Current State Analysis

### Issues Identified

1. **Incomplete `param_to_arg()` function** in `src/cli/parser.rs` (lines 664-680)
   - Missing handling for `ParamType::FLG` boolean flags
   - All parameters treated as string values regardless of type
   - Boolean flags don't use `clap::ArgAction::SetTrue`

2. **Incomplete boolean flag value extraction** (lines 590-596)
   - Only retrieves `String` values with `get_one::<String>()`
   - Boolean flags need `get_flag()` for proper true/false values
   - Missing default value handling for absent flags

3. **Missing comprehensive unit tests**
   - No dedicated tests for parameter parsing logic
   - No tests for boolean vs argument flag differentiation
   - No tests for YAML configuration parsing

## Implementation Plan

### Phase 1: Fix Core Parameter Handling

#### 1.1 Fix `param_to_arg()` Function
**File:** `src/cli/parser.rs` (lines 664-680)

**Current:**
```rust
fn param_to_arg(param_spec: &ParamSpec) -> Arg {
    let mut arg = Arg::new(param_spec.name.clone());
    // ... setup short/long/help
    arg.value_parser(value_parser!(String))  // WRONG: treats all as strings
}
```

**Fixed:**
```rust
fn param_to_arg(param_spec: &ParamSpec) -> Arg {
    let mut arg = Arg::new(param_spec.name.clone());

    if let Some(short) = param_spec.short {
        arg = arg.short(short);
    }

    if let Some(ref long) = param_spec.long {
        arg = arg.long(long.clone());
    }

    if let Some(ref help) = param_spec.help {
        arg = arg.help(help.clone());
    }

    // Handle different parameter types
    match param_spec.param_type {
        ParamType::FLG => {
            // Boolean flag - no value required
            arg = arg.action(clap::ArgAction::SetTrue);
        }
        ParamType::OPT | ParamType::POS => {
            // Argument with value
            arg = arg.value_parser(value_parser!(String));

            // Add default value if specified
            if let Some(ref default) = param_spec.default {
                arg = arg.default_value(default.clone());
            }

            // Add choices validation if specified
            if !param_spec.choices.is_empty() {
                arg = arg.value_parser(clap::builder::PossibleValuesParser::new(&param_spec.choices));
            }
        }
    }

    // Handle positional arguments
    if param_spec.param_type == ParamType::POS {
        arg = arg.value_name(
            param_spec.metavar.as_deref()
                .unwrap_or_else(|| param_spec.name.as_str())
        );
    }

    arg
}
```

#### 1.2 Fix Parameter Value Extraction
**File:** `src/cli/parser.rs` (lines 590-596)

**Current:**
```rust
for param_spec in task_spec.params.values() {
    if let Some(value) = matches.get_one::<String>(param_spec.name.as_str()) {
        task.values.insert(param_spec.name.clone(), Value::Item(value.to_string()));
        task.envs.insert(param_spec.name.clone(), value.to_string());
    }
}
```

**Fixed:**
```rust
for param_spec in task_spec.params.values() {
    match param_spec.param_type {
        ParamType::FLG => {
            // Boolean flag - use get_flag()
            let flag_value = matches.get_flag(param_spec.name.as_str());
            let value_str = if flag_value { "true" } else { "false" };

            task.values.insert(param_spec.name.clone(), Value::Item(value_str.to_string()));
            task.envs.insert(param_spec.name.clone(), value_str.to_string());
        }
        ParamType::OPT | ParamType::POS => {
            // Argument with value - use get_one::<String>()
            if let Some(value) = matches.get_one::<String>(param_spec.name.as_str()) {
                task.values.insert(param_spec.name.clone(), Value::Item(value.to_string()));
                task.envs.insert(param_spec.name.clone(), value.to_string());
            } else if let Some(ref default) = param_spec.default {
                // Apply default value if not provided
                task.values.insert(param_spec.name.clone(), Value::Item(default.clone()));
                task.envs.insert(param_spec.name.clone(), default.clone());
            }
        }
    }
}
```

### Phase 2: Enhance Parameter Type Detection

#### 2.1 Improve Boolean Detection Logic
**File:** `src/cfg/param.rs` (lines 239-247)

**Current logic is correct but could be more robust:**
```rust
if param_spec.long.is_some() || param_spec.short.is_some() {
    if let Some(ref value) = param_spec.default {
        if value == "true" || value == "false" {
            param_spec.param_type = ParamType::FLG;
        }
    }
} else {
    param_spec.param_type = ParamType::POS;
}
```

**Enhanced:**
```rust
if param_spec.long.is_some() || param_spec.short.is_some() {
    if let Some(ref value) = param_spec.default {
        // Case-insensitive boolean detection
        let lower_value = value.to_lowercase();
        if lower_value == "true" || lower_value == "false" {
            param_spec.param_type = ParamType::FLG;
        }
    }
    // If no default specified, check if it's explicitly a boolean constant
    if param_spec.param_type != ParamType::FLG {
        match param_spec.constant {
            Value::Item(ref val) => {
                let lower_val = val.to_lowercase();
                if lower_val == "true" || lower_val == "false" {
                    param_spec.param_type = ParamType::FLG;
                }
            }
            _ => {}
        }
    }
} else {
    param_spec.param_type = ParamType::POS;
}
```

### Phase 3: Comprehensive Unit Tests

#### 3.1 Parameter Configuration Tests
**File:** `src/cfg/param.rs` (add tests module)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_boolean_flag_detection_true_default() {
        let yaml = r#"
        -v|--verbose:
          default: true
          help: Enable verbose output
        "#;

        let params: ParamSpecs = serde_yaml::from_str(yaml).unwrap();
        let verbose = params.get("verbose").unwrap();

        assert_eq!(verbose.param_type, ParamType::FLG);
        assert_eq!(verbose.short, Some('v'));
        assert_eq!(verbose.long, Some("verbose".to_string()));
        assert_eq!(verbose.default, Some("true".to_string()));
    }

    #[test]
    fn test_boolean_flag_detection_false_default() {
        let yaml = r#"
        --debug:
          default: false
          help: Enable debug mode
        "#;

        let params: ParamSpecs = serde_yaml::from_str(yaml).unwrap();
        let debug = params.get("debug").unwrap();

        assert_eq!(debug.param_type, ParamType::FLG);
        assert_eq!(debug.short, None);
        assert_eq!(debug.long, Some("debug".to_string()));
        assert_eq!(debug.default, Some("false".to_string()));
    }

    #[test]
    fn test_argument_flag_with_choices() {
        let yaml = r#"
        -e|--env:
          default: development
          choices: [development, staging, production]
          help: Target environment
        "#;

        let params: ParamSpecs = serde_yaml::from_str(yaml).unwrap();
        let env = params.get("env").unwrap();

        assert_eq!(env.param_type, ParamType::OPT);
        assert_eq!(env.choices, vec!["development", "staging", "production"]);
        assert_eq!(env.default, Some("development".to_string()));
    }

    #[test]
    fn test_positional_parameter() {
        let yaml = r#"
        filename:
          help: Input filename
        "#;

        let params: ParamSpecs = serde_yaml::from_str(yaml).unwrap();
        let filename = params.get("filename").unwrap();

        assert_eq!(filename.param_type, ParamType::POS);
        assert_eq!(filename.short, None);
        assert_eq!(filename.long, None);
    }

    #[test]
    fn test_mixed_parameters() {
        let yaml = r#"
        -v|--verbose:
          default: false
          help: Enable verbose output
        -e|--env:
          default: development
          choices: [development, staging, production]
          help: Target environment
        input_file:
          help: Input file path
        "#;

        let params: ParamSpecs = serde_yaml::from_str(yaml).unwrap();

        assert_eq!(params.get("verbose").unwrap().param_type, ParamType::FLG);
        assert_eq!(params.get("env").unwrap().param_type, ParamType::OPT);
        assert_eq!(params.get("input_file").unwrap().param_type, ParamType::POS);
    }
}
```

#### 3.2 CLI Parser Tests
**File:** `src/cli/parser.rs` (add tests module)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::task::TaskSpec;
    use crate::cfg::param::{ParamSpec, ParamType};
    use clap::Command;

    fn create_test_param_spec(name: &str, param_type: ParamType, short: Option<char>, long: Option<&str>) -> ParamSpec {
        ParamSpec {
            name: name.to_string(),
            short,
            long: long.map(|s| s.to_string()),
            param_type,
            dest: None,
            metavar: None,
            default: match param_type {
                ParamType::FLG => Some("false".to_string()),
                _ => None,
            },
            constant: Value::Empty,
            choices: vec![],
            nargs: Nargs::default(),
            help: Some(format!("Help for {}", name)),
            value: Value::Empty,
        }
    }

    #[test]
    fn test_param_to_arg_boolean_flag() {
        let param = create_test_param_spec("verbose", ParamType::FLG, Some('v'), Some("verbose"));
        let arg = Parser::param_to_arg(&param);

        // Test that the argument is configured correctly for boolean flags
        let cmd = Command::new("test").arg(arg);
        let matches = cmd.try_get_matches_from(vec!["test", "--verbose"]).unwrap();

        assert!(matches.get_flag("verbose"));
    }

    #[test]
    fn test_param_to_arg_string_argument() {
        let mut param = create_test_param_spec("env", ParamType::OPT, Some('e'), Some("env"));
        param.choices = vec!["dev".to_string(), "prod".to_string()];
        param.default = Some("dev".to_string());

        let arg = Parser::param_to_arg(&param);
        let cmd = Command::new("test").arg(arg);

        // Test with explicit value
        let matches = cmd.try_get_matches_from(vec!["test", "--env", "prod"]).unwrap();
        assert_eq!(matches.get_one::<String>("env").unwrap(), "prod");

        // Test with default (need to handle this in clap setup)
        let matches = cmd.try_get_matches_from(vec!["test"]).unwrap();
        assert_eq!(matches.get_one::<String>("env").unwrap_or(&"dev".to_string()), "dev");
    }

    #[test]
    fn test_param_to_arg_positional() {
        let param = create_test_param_spec("filename", ParamType::POS, None, None);
        let arg = Parser::param_to_arg(&param);

        let cmd = Command::new("test").arg(arg);
        let matches = cmd.try_get_matches_from(vec!["test", "input.txt"]).unwrap();

        assert_eq!(matches.get_one::<String>("filename").unwrap(), "input.txt");
    }

    #[test]
    fn test_task_command_generation() {
        let mut task_spec = TaskSpec::default();
        task_spec.name = "build".to_string();
        task_spec.help = Some("Build the project".to_string());

        // Add boolean flag
        let mut verbose_param = create_test_param_spec("verbose", ParamType::FLG, Some('v'), Some("verbose"));
        task_spec.params.insert("verbose".to_string(), verbose_param);

        // Add argument flag
        let mut env_param = create_test_param_spec("env", ParamType::OPT, Some('e'), Some("env"));
        env_param.default = Some("development".to_string());
        task_spec.params.insert("env".to_string(), env_param);

        let cmd = Parser::task_to_command(&task_spec);

        // Test boolean flag
        let matches = cmd.try_get_matches_from(vec!["build", "--verbose", "--env", "production"]).unwrap();
        assert!(matches.get_flag("verbose"));
        assert_eq!(matches.get_one::<String>("env").unwrap(), "production");
    }
}
```

#### 3.3 Integration Tests
**File:** `tests/flag_integration_test.rs`

```rust
use otto::cli::parser::Parser;
use otto::cfg::config::ConfigSpec;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_boolean_flags_integration() {
    let temp_dir = TempDir::new().unwrap();
    let otto_file = temp_dir.path().join("otto.yml");

    let config = r#"
otto:
  api: 1
  tasks: [build]

tasks:
  build:
    params:
      -v|--verbose:
        default: false
        help: Enable verbose output
      -f|--force:
        default: false
        help: Force rebuild
    bash: |
      echo "Verbose: ${verbose}"
      echo "Force: ${force}"
    "#;

    fs::write(&otto_file, config).unwrap();

    // Test with flags present
    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "build".to_string(),
        "--verbose".to_string(),
        "--force".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let (tasks, _, _) = parser.parse().unwrap();

    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.name, "build");
    assert_eq!(task.envs.get("verbose").unwrap(), "true");
    assert_eq!(task.envs.get("force").unwrap(), "true");
}

#[test]
fn test_argument_flags_integration() {
    let temp_dir = TempDir::new().unwrap();
    let otto_file = temp_dir.path().join("otto.yml");

    let config = r#"
otto:
  api: 1
  tasks: [deploy]

tasks:
  deploy:
    params:
      -e|--env:
        default: development
        choices: [development, staging, production]
        help: Target environment
      --timeout:
        default: 30
        help: Timeout in seconds
    bash: |
      echo "Environment: ${env}"
      echo "Timeout: ${timeout}"
    "#;

    fs::write(&otto_file, config).unwrap();

    // Test with explicit values
    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "deploy".to_string(),
        "--env".to_string(),
        "production".to_string(),
        "--timeout".to_string(),
        "60".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let (tasks, _, _) = parser.parse().unwrap();

    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.name, "deploy");
    assert_eq!(task.envs.get("env").unwrap(), "production");
    assert_eq!(task.envs.get("timeout").unwrap(), "60");
}

#[test]
fn test_mixed_flags_integration() {
    let temp_dir = TempDir::new().unwrap();
    let otto_file = temp_dir.path().join("otto.yml");

    let config = r#"
otto:
  api: 1
  tasks: [test]

tasks:
  test:
    params:
      -v|--verbose:
        default: false
        help: Enable verbose output
      --coverage:
        default: false
        help: Generate coverage report
      -p|--pattern:
        default: "**/*.test.js"
        help: Test file pattern
      --reporter:
        choices: [spec, json, junit]
        default: spec
        help: Test reporter format
    bash: |
      echo "Verbose: ${verbose}"
      echo "Coverage: ${coverage}"
      echo "Pattern: ${pattern}"
      echo "Reporter: ${reporter}"
    "#;

    fs::write(&otto_file, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "test".to_string(),
        "--verbose".to_string(),
        "--coverage".to_string(),
        "--pattern".to_string(),
        "src/**/*.test.js".to_string(),
        "--reporter".to_string(),
        "json".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let (tasks, _, _) = parser.parse().unwrap();

    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.name, "test");
    assert_eq!(task.envs.get("verbose").unwrap(), "true");
    assert_eq!(task.envs.get("coverage").unwrap(), "true");
    assert_eq!(task.envs.get("pattern").unwrap(), "src/**/*.test.js");
    assert_eq!(task.envs.get("reporter").unwrap(), "json");
}

#[test]
fn test_default_values_integration() {
    let temp_dir = TempDir::new().unwrap();
    let otto_file = temp_dir.path().join("otto.yml");

    let config = r#"
otto:
  api: 1
  tasks: [serve]

tasks:
  serve:
    params:
      -v|--verbose:
        default: false
        help: Enable verbose output
      -p|--port:
        default: 3000
        help: Port number
      --host:
        default: localhost
        help: Host address
    bash: |
      echo "Verbose: ${verbose}"
      echo "Port: ${port}"
      echo "Host: ${host}"
    "#;

    fs::write(&otto_file, config).unwrap();

    // Test with no flags (should use defaults)
    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        otto_file.to_string_lossy().to_string(),
        "serve".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let (tasks, _, _) = parser.parse().unwrap();

    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.name, "serve");
    assert_eq!(task.envs.get("verbose").unwrap(), "false");
    assert_eq!(task.envs.get("port").unwrap(), "3000");
    assert_eq!(task.envs.get("host").unwrap(), "localhost");
}
```

### Phase 4: Error Handling and Validation

#### 4.1 Add Parameter Validation
**File:** `src/cfg/param.rs`

```rust
impl ParamSpec {
    pub fn validate_value(&self, value: &str) -> Result<(), String> {
        // Validate choices
        if !self.choices.is_empty() && !self.choices.contains(&value.to_string()) {
            return Err(format!(
                "Invalid value '{}' for parameter '{}'. Valid choices are: {}",
                value,
                self.name,
                self.choices.join(", ")
            ));
        }

        // Additional validation can be added here
        Ok(())
    }
}
```

#### 4.2 Add Better Error Messages
**File:** `src/cli/error.rs`

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ParameterError {
    #[error("Invalid value '{value}' for parameter '{param}': {reason}")]
    InvalidValue {
        param: String,
        value: String,
        reason: String,
    },

    #[error("Missing required parameter '{param}'")]
    MissingRequired { param: String },

    #[error("Parameter '{param}' cannot be used with boolean flag syntax")]
    BooleanSyntaxError { param: String },
}
```

### Phase 5: Documentation and Examples

#### 5.1 Update Examples
Create comprehensive examples in `examples/flags/`:

- `examples/flags/boolean-flags.yml` - Pure boolean flag usage
- `examples/flags/argument-flags.yml` - Argument flags with validation
- `examples/flags/mixed-flags.yml` - Combined usage patterns
- `examples/flags/advanced-flags.yml` - Complex validation and choices

#### 5.2 Update Help Generation
Ensure help output clearly distinguishes between boolean flags and argument flags.

## Implementation Timeline

### Week 1: Core Fixes
- [ ] Fix `param_to_arg()` function
- [ ] Fix parameter value extraction logic
- [ ] Basic unit tests for parameter type detection

### Week 2: Comprehensive Testing
- [ ] Complete unit test suite for `src/cfg/param.rs`
- [ ] Complete unit test suite for `src/cli/parser.rs`
- [ ] Integration tests for end-to-end flag handling

### Week 3: Error Handling & Validation
- [ ] Add parameter validation logic
- [ ] Improve error messages
- [ ] Add validation tests

### Week 4: Documentation & Polish
- [ ] Create comprehensive examples
- [ ] Update documentation
- [ ] Performance testing and optimization

## Success Criteria

1. **Functionality**
   - [ ] Boolean flags work correctly (`--verbose` sets `verbose=true`)
   - [ ] Argument flags work correctly (`--env production` sets `env=production`)
   - [ ] Default values are applied correctly
   - [ ] Choices validation works
   - [ ] Mixed flag types work together

2. **Testing**
   - [ ] >95% code coverage for parameter handling
   - [ ] All edge cases covered
   - [ ] Integration tests pass
   - [ ] Performance tests show no regression

3. **User Experience**
   - [ ] Clear error messages for invalid usage
   - [ ] Comprehensive help output
   - [ ] Intuitive YAML configuration
   - [ ] Good documentation with examples

## Risk Mitigation

1. **Backwards Compatibility**
   - Ensure existing configurations continue to work
   - Add deprecation warnings for any breaking changes
   - Provide migration guide if needed

2. **Performance**
   - Profile parameter parsing performance
   - Ensure no significant slowdown for large configurations
   - Optimize hot paths if needed

3. **Edge Cases**
   - Test with empty configurations
   - Test with malformed YAML
   - Test with conflicting parameter names
   - Test with very long parameter lists

This plan provides a comprehensive roadmap for fully implementing flag support in Otto with robust testing and documentation.
