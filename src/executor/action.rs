use crate::ports::FileSystem;
use eyre::{Result, eyre};
use hex;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

use super::task::Task;
use super::workspace::Workspace;

/// Quote `value` as a bash single-quoted word.
///
/// Values are data, never script. Templating them into `export K="$v"` made
/// every env value, parameter, foreach item and dynamic choices value a shell
/// injection: `x"; touch /tmp/pwned; echo "y` closed the quote and ran. Single
/// quotes suppress every expansion bash performs, and the one byte they cannot
/// carry - `'` itself - is escaped as `'\''` exactly as `otto_serialize_output`
/// already does when it writes the same kind of value back out.
fn bash_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Quote `value` as a single-quoted Python string literal.
///
/// Same contract as `bash_quote`: backslash first (so the escapes added after
/// it are not themselves escaped), then the quote character, then the control
/// bytes that would otherwise end the literal at a line break.
fn python_quote(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("'{escaped}'")
}

/// Reject a name that cannot appear on the left of an assignment in the
/// generated script.
///
/// Quoting protects the value; the name is on the other side of the `=` and no
/// quoting reaches it, so a key like `X; touch /tmp/pwned` would still execute.
/// Ottofile `envs:` keys are free-form YAML, so this is a real surface, and a
/// loud config-time error beats a script that runs something the author did not
/// write.
fn validate_identifier(kind: &str, task_name: &str, name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(eyre!(
            "task '{task_name}': {kind} name '{name}' is not a valid identifier \
             (letters, digits and underscore only, not starting with a digit)"
        ))
    }
}

/// Flatten a parameter value to the single string the script sees.
fn param_value_to_string(value: &crate::cfg::param::Value) -> String {
    match value {
        crate::cfg::param::Value::Item(s) => s.clone(),
        crate::cfg::param::Value::List(l) => l.join(" "),
        crate::cfg::param::Value::Dict(d) => {
            // Convert dict to space-separated key=value pairs
            let mut pairs: Vec<String> = d.iter().map(|(k, v)| format!("{k}={v}")).collect();
            pairs.sort();
            pairs.join(" ")
        }
        crate::cfg::param::Value::Empty => String::new(),
    }
}

/// Key/value pairs in a stable order.
///
/// The generated script is content-addressed, so HashMap iteration order meant
/// one unchanged ottofile produced a different script hash - and a different
/// cache file - on every run.
fn sorted_pairs<V>(map: &std::collections::HashMap<String, V>) -> Vec<(&String, &V)> {
    let mut pairs: Vec<(&String, &V)> = map.iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    pairs
}

/// Processed action with script type encoded in the enum variant
pub enum ProcessedAction {
    Bash {
        path: PathBuf,
        script: String,
        hash: String,
    },
    Python3 {
        path: PathBuf,
        script: String,
        hash: String,
    },
}

/// Main coordinator for action processing
pub struct ActionProcessor<F: FileSystem = crate::ports::RealFs> {
    workspace: Arc<Workspace<F>>,
    task_name: String,
}

impl<F: FileSystem> ActionProcessor<F> {
    pub fn new(workspace: Arc<Workspace<F>>, task_name: &str) -> Result<Self> {
        Ok(Self {
            workspace,
            task_name: task_name.to_string(),
        })
    }

    pub fn process(&self, user_action: &str, task: &Task) -> Result<ProcessedAction> {
        let trimmed_action = user_action.trim_start();

        // Detect script language from shebang
        if trimmed_action.starts_with("#!/usr/bin/env bash") || trimmed_action.starts_with("#!/bin/bash") {
            let processor = BashProcessor::new(self.workspace.clone(), &self.task_name);
            processor.create_builtins()?;
            let script = self.build_script(&processor, user_action, task)?;
            let (path, hash) = self.write_script(&processor, &script)?;
            Ok(ProcessedAction::Bash { path, script, hash })
        } else if trimmed_action.starts_with("#!/usr/bin/env python3")
            || trimmed_action.starts_with("#!/usr/bin/python3")
        {
            let processor = PythonProcessor::new(self.workspace.clone(), &self.task_name);
            processor.create_builtins()?;
            let script = self.build_script(&processor, user_action, task)?;
            let (path, hash) = self.write_script(&processor, &script)?;
            Ok(ProcessedAction::Python3 { path, script, hash })
        } else {
            // Default to bash if no shebang is detected (for backward compatibility)
            let processor = BashProcessor::new(self.workspace.clone(), &self.task_name);
            processor.create_builtins()?;
            let script = self.build_script(&processor, user_action, task)?;
            let (path, hash) = self.write_script(&processor, &script)?;
            Ok(ProcessedAction::Bash { path, script, hash })
        }
    }

    fn build_script<T: ScriptProcessor>(&self, processor: &T, user_action: &str, task: &Task) -> Result<String> {
        log::debug!(
            "build_script: task={} interpreter={} deps={}",
            self.task_name,
            processor.get_interpreter(),
            task.task_deps.len()
        );
        // Extract shebang from user action if present
        let lines: Vec<&str> = user_action.lines().collect();
        let (shebang, user_content) = if lines.first().is_some_and(|line| line.starts_with("#!")) {
            (lines[0], lines[1..].join("\n"))
        } else {
            ("", user_action.to_string())
        };

        let dep_names: Vec<String> = task.task_deps.iter().map(|e| e.task.clone()).collect();
        let prologue = processor.generate_prologue(&dep_names, task)?;
        let epilogue = processor.generate_epilogue()?;

        let script = if shebang.is_empty() {
            format!("{prologue}\n{user_content}\n{epilogue}")
        } else {
            format!("{shebang}\n{prologue}\n{user_content}\n{epilogue}")
        };

        Ok(script)
    }

    /// Writes the cache entry and task-dir symlink, returning both the
    /// symlink path and the content hash it was keyed on so the caller does
    /// not have to recompute the same hash a second time.
    fn write_script<T: ScriptProcessor>(&self, processor: &T, script: &str) -> Result<(PathBuf, String)> {
        // Calculate hash for caching
        let hash = self.calculate_hash(script)?;
        log::debug!(
            "write_script: task={} hash={hash} script_bytes={}",
            self.task_name,
            script.len()
        );

        // Write to cache directory
        let cache_file = self
            .workspace
            .cache_dir()
            .join(format!("{}.{}", hash, processor.get_file_extension()));

        // Ensure cache directory exists
        self.workspace.fs().create_dir_all_sync(self.workspace.cache_dir())?;

        // Write the script to the cache unless a byte-identical copy is already
        // there. "Exists" is not good enough: the cache is content-addressed, so a
        // torn or truncated entry keeps its name, and write-only-if-absent then
        // re-executed that stump on every future run - successfully, since a
        // truncated bash script still exits 0. Validating the hit is what turns a
        // poisoned cache entry into a self-healing one.
        let needs_write = match self.workspace.fs().read_sync(&cache_file) {
            Ok(cached) => cached != script.as_bytes(),
            Err(_) => true,
        };
        if needs_write {
            self.workspace.fs().write_sync(&cache_file, script.as_bytes())?;

            // Make cached script executable
            #[cfg(unix)]
            {
                self.workspace.fs().set_permissions_sync(&cache_file, 0o755)?;
            }
        }

        let script_path = self
            .workspace
            .task_script_file(&self.task_name, processor.get_file_extension());

        // Ensure task directory exists
        if let Some(parent) = script_path.parent() {
            self.workspace.fs().create_dir_all_sync(parent)?;
        }

        if self.workspace.fs().exists_sync(&script_path) {
            self.workspace.fs().remove_file_sync(&script_path)?;
        }

        #[cfg(unix)]
        {
            // Use relative path for portability
            let relative_cache = self.workspace.relative_script_cache_path(&cache_file);
            self.workspace.fs().symlink_sync(&relative_cache, &script_path)?;
        }
        #[cfg(not(unix))]
        {
            // Fallback: copy file on non-Unix systems
            self.workspace.fs().copy_sync(&cache_file, &script_path)?;
        }

        Ok((script_path, hash))
    }

    fn calculate_hash(&self, script: &str) -> Result<String> {
        let mut hasher = Sha256::new();
        hasher.update(script.as_bytes());
        let hash = hasher.finalize();
        Ok(hex::encode(hash)[..8].to_string())
    }
}

/// Trait for script-specific processing logic
pub trait ScriptProcessor {
    fn generate_prologue(&self, dependencies: &[String], task: &Task) -> Result<String>;

    fn generate_epilogue(&self) -> Result<String>;

    fn get_interpreter(&self) -> &str;

    fn get_file_extension(&self) -> &str;

    fn create_builtins(&self) -> Result<()>;
}

/// Bash script processor
pub struct BashProcessor<F: FileSystem = crate::ports::RealFs> {
    workspace: Arc<Workspace<F>>,
    task_name: String,
}

impl<F: FileSystem> BashProcessor<F> {
    pub fn new(workspace: Arc<Workspace<F>>, task_name: &str) -> Self {
        Self {
            workspace,
            task_name: task_name.to_string(),
        }
    }

    /// Separate environment variables from CLI parameters
    /// Only include variables that don't exist in task.values (CLI parameters)
    fn get_yaml_env_vars(&self, task: &Task) -> std::collections::HashMap<String, String> {
        let mut yaml_envs = std::collections::HashMap::new();
        for (key, value) in &task.envs {
            // Include all environment variables - CLI parameters are handled separately
            // The task.envs already contains both global and task-level env vars
            yaml_envs.insert(key.clone(), value.clone());
        }
        yaml_envs
    }

    fn generate_bash_env_section(&self, task: &Task) -> Result<String> {
        let yaml_envs = self.get_yaml_env_vars(task);
        if yaml_envs.is_empty() {
            return Ok(String::new());
        }

        let mut env_exports = vec![
            "# Environment Variables".to_string(),
            "################################################################################".to_string(),
        ];

        // Export only actual environment variables from YAML (not CLI parameters).
        // Values are already fully resolved by `cfg::env`, so there is nothing left
        // for the shell to expand and every byte is literal text.
        for (key, value) in sorted_pairs(&yaml_envs) {
            validate_identifier("environment variable", &self.task_name, key)?;
            env_exports.push(format!("export {key}={}", bash_quote(value)));
        }

        env_exports.push(String::new()); // Add blank line after section
        Ok(env_exports.join("\n"))
    }

    fn generate_bash_input_section(&self, dependencies: &[String]) -> String {
        if dependencies.is_empty() {
            return String::new();
        }

        let mut input_section = vec![
            "# Input Loading".to_string(),
            "################################################################################".to_string(),
        ];

        // Use builtins functions for deserialization. A task name is data here
        // too: a foreach subtask is named after its item, so an item containing a
        // quote used to end the argument and break the script.
        for dep in dependencies {
            input_section.push(format!("otto_deserialize_input {}", bash_quote(dep)));
        }

        input_section.push(String::new()); // Add blank line after section
        input_section.join("\n")
    }

    fn generate_bash_param_section(&self, task: &Task) -> Result<String> {
        if task.values.is_empty() {
            return Ok(String::new());
        }

        let mut param_section = vec![
            "# Parameter Assignments".to_string(),
            "################################################################################".to_string(),
        ];

        // Simple parameter assignments for CLI parameters only
        for (param_name, param_value) in sorted_pairs(&task.values) {
            let value_str = param_value_to_string(param_value);
            // Convert hyphens to underscores for valid bash variable names
            let bash_var_name = param_name.replace('-', "_");
            validate_identifier("parameter", &self.task_name, &bash_var_name)?;
            param_section.push(format!("{bash_var_name}={}", bash_quote(&value_str)));
        }

        param_section.push(String::new()); // Add blank line after section
        Ok(param_section.join("\n"))
    }
}

impl<F: FileSystem> ScriptProcessor for BashProcessor<F> {
    fn generate_prologue(&self, dependencies: &[String], task: &Task) -> Result<String> {
        let env_section = self.generate_bash_env_section(task)?;
        let input_section = self.generate_bash_input_section(dependencies);
        let param_section = self.generate_bash_param_section(task)?;

        let prologue = format!(
            r#"# Otto-generated bash prologue
set -euo pipefail

declare -a OTTO_INPUT
declare -a OTTO_OUTPUT

# Set Otto environment variables
export OTTO_TASK_DIR="$(dirname "$0")"

# Source Otto builtins
source "$(dirname "$0")/builtins.sh"

{env_section}
{input_section}
{param_section}"#
        );
        Ok(prologue)
    }

    fn generate_epilogue(&self) -> Result<String> {
        let epilogue = format!(
            "\n# Output Serialization\n\
             ################################################################################\n\
             # Serialize OTTO_OUTPUT to output.<task>.env using builtins\n\
             otto_serialize_output {}\n",
            bash_quote(&self.task_name)
        );
        Ok(epilogue)
    }

    fn get_interpreter(&self) -> &str {
        "bash"
    }

    fn get_file_extension(&self) -> &str {
        "sh"
    }

    fn create_builtins(&self) -> Result<()> {
        let builtins_path = self.workspace.task_dir(&self.task_name).join("builtins.sh");

        // Ensure task directory exists before writing builtins
        if let Some(parent) = builtins_path.parent() {
            self.workspace.fs().create_dir_all_sync(parent)?;
        }

        let builtins_content = r##"#!/bin/bash
# Otto Bash Builtins
# Functions to handle input/output file serialization
# Compatible with Bash 3.2+ (uses indexed arrays instead of associative arrays)
# NO JQ REQUIRED - uses .env files generated/consumed by Otto (Rust)

# ANSI Color Codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
MAGENTA='\033[0;35m'
CYAN='\033[0;36m'
WHITE='\033[0;37m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'  # No Color / Reset

# Function to deserialize input.<task-name>.env -> OTTO_INPUT
# Otto generates .env files from dependency JSON outputs before task runs
otto_deserialize_input() {
    local task_name="$1"
    local env_file="$OTTO_TASK_DIR/input.${task_name}.env"

    if [ -f "$env_file" ]; then
        # Source the .env file to load OTTO_INPUT_<TASK>_<KEY> variables
        # shellcheck disable=SC1090
        source "$env_file"

        # Build the expected variable prefix: OTTO_INPUT_<TASK>_
        local task_upper
        task_upper=$(echo "$task_name" | tr '[:lower:]-' '[:upper:]_')
        local prefix="OTTO_INPUT_${task_upper}_"
        local prefix_len=${#prefix}

        # Also populate OTTO_INPUT array for backward compatibility.
        #
        # Read the values back out of the shell, not out of the file: the `source`
        # above already parsed it, quotes, escapes, embedded newlines and all.
        # Re-parsing the text here with a line-based loop was a second parser that
        # disagreed with the first, and it silently dropped every value containing
        # a newline - the record spans lines, so no single line matched KEY='value'.
        local var_name key
        while IFS= read -r var_name; do
            case "$var_name" in
                "${prefix}"*) ;;
                *) continue ;;
            esac
            # Strip the prefix and convert back to lowercase with underscores
            key="${var_name:$prefix_len}"
            key=$(echo "$key" | tr '[:upper:]' '[:lower:]')
            OTTO_INPUT+=("${task_name}.${key}=${!var_name}")
        done < <(compgen -v)
    fi
}

# Function to serialize OTTO_OUTPUT -> output.<task-name>.env
# Otto will convert this .env file to JSON after task completes
otto_serialize_output() {
    local task_name="$1"
    local env_file="$OTTO_TASK_DIR/output.${task_name}.env"

    # Write header
    {
        echo "# Auto-generated by Otto task: ${task_name}"
        echo "# Otto will convert this to JSON after task completion"
        echo ""
    } > "$env_file"

    # Check if OTTO_OUTPUT has any items (safely handle set -u)
    set +u  # Temporarily disable unbound variable check
    local output_count="${#OTTO_OUTPUT[@]}"

    if [ "$output_count" -gt 0 ]; then
        # Iterate through indexed array items (format: key=value)
        for item in "${OTTO_OUTPUT[@]}"; do
            local key="${item%%=*}"    # Extract key (everything before first =)
            local value="${item#*=}"   # Extract value (everything after first =)

            # Escape single quotes for bash single-quoted string
            local escaped="${value//\'/\'\\\'\'}"

            # Write key='escaped_value'
            echo "${key}='${escaped}'" >> "$env_file"
        done
    fi
    set -u  # Re-enable unbound variable check
}

# Helper function to get input value by key
# Uses linear search through indexed array (Bash 3.2 compatible)
otto_get_input() {
    local key="$1"
    local result=""

    # Safely search through array (handles empty array with set -u)
    set +u  # Temporarily disable for array operations
    if [ "${#OTTO_INPUT[@]}" -gt 0 ]; then
        for item in "${OTTO_INPUT[@]}"; do
            if [[ "$item" == "$key="* ]]; then
                result="${item#*=}"  # Extract value after first =
                break
            fi
        done
    fi
    set -u  # Re-enable after array operations

    echo "$result"
}

# Helper function to set output value by key
# Replaces existing key if present, otherwise appends (Bash 3.2 compatible)
otto_set_output() {
    local key="$1"
    local value="$2"

    # Remove existing key if present (to allow updates)
    local new_array=()
    local has_items=false

    # Safely iterate through array (handles empty array with set -u)
    set +u  # Temporarily disable for array operations
    if [ "${#OTTO_OUTPUT[@]}" -gt 0 ]; then
        for item in "${OTTO_OUTPUT[@]}"; do
            if [[ "$item" != "$key="* ]]; then
                new_array+=("$item")
                has_items=true
            fi
        done
    fi

    # Add new key-value pair
    new_array+=("$key=$value")

    # Reassign array safely
    if [ "${#new_array[@]}" -gt 0 ]; then
        OTTO_OUTPUT=("${new_array[@]}")
    else
        OTTO_OUTPUT=()
    fi
    set -u  # Re-enable after array operations
}
"##;
        self.workspace
            .fs()
            .write_sync(&builtins_path, builtins_content.as_bytes())?;

        // Make builtins executable
        #[cfg(unix)]
        {
            self.workspace.fs().set_permissions_sync(&builtins_path, 0o755)?;
        }

        Ok(())
    }
}

/// Python script processor
pub struct PythonProcessor<F: FileSystem = crate::ports::RealFs> {
    workspace: Arc<Workspace<F>>,
    task_name: String,
}

impl<F: FileSystem> PythonProcessor<F> {
    pub fn new(workspace: Arc<Workspace<F>>, task_name: &str) -> Self {
        Self {
            workspace,
            task_name: task_name.to_string(),
        }
    }

    /// Separate environment variables from CLI parameters
    /// Only include variables that don't exist in task.values (CLI parameters)
    fn get_yaml_env_vars(&self, task: &Task) -> std::collections::HashMap<String, String> {
        let mut yaml_envs = std::collections::HashMap::new();
        for (key, value) in &task.envs {
            // Include all environment variables - CLI parameters are handled separately
            // The task.envs already contains both global and task-level env vars
            yaml_envs.insert(key.clone(), value.clone());
        }
        yaml_envs
    }

    fn generate_python_env_section(&self, task: &Task) -> Result<String> {
        let yaml_envs = self.get_yaml_env_vars(task);
        if yaml_envs.is_empty() {
            return Ok(String::new());
        }

        let mut env_section = vec![
            "# Environment Variables".to_string(),
            "################################################################################".to_string(),
        ];

        // Set only actual environment variables from YAML (not CLI parameters).
        // The value is a literal string, never Python source.
        for (key, value) in sorted_pairs(&yaml_envs) {
            validate_identifier("environment variable", &self.task_name, key)?;
            env_section.push(format!(
                "os.environ['{}'] = {}",
                key.to_uppercase(),
                python_quote(value)
            ));
        }

        env_section.push(String::new()); // Add blank line after section
        Ok(env_section.join("\n"))
    }

    fn generate_python_input_section(&self, dependencies: &[String]) -> String {
        if dependencies.is_empty() {
            return String::new();
        }

        let mut input_section = vec![
            "# Input Loading".to_string(),
            "################################################################################".to_string(),
        ];

        // Use builtins functions for deserialization; the name is data, not source.
        for dep in dependencies {
            input_section.push(format!("otto_deserialize_input({})", python_quote(dep)));
        }

        input_section.push(String::new()); // Add blank line after section
        input_section.join("\n")
    }

    fn generate_python_param_section(&self, task: &Task) -> Result<String> {
        if task.values.is_empty() {
            return Ok(String::new());
        }

        let mut param_section = vec![
            "# Parameter Assignments".to_string(),
            "################################################################################".to_string(),
        ];

        // Simple parameter assignments for CLI parameters only
        for (param_name, param_value) in sorted_pairs(&task.values) {
            let value_str = param_value_to_string(param_value);
            // Same hyphen rule bash gets: `--dry-run` binds as `dry_run` rather
            // than emitting `dry-run = '...'`, which is a Python syntax error.
            let py_var_name = param_name.replace('-', "_");
            validate_identifier("parameter", &self.task_name, &py_var_name)?;
            param_section.push(format!("{py_var_name} = {}", python_quote(&value_str)));
        }

        param_section.push(String::new()); // Add blank line after section
        Ok(param_section.join("\n"))
    }
}

impl<F: FileSystem> ScriptProcessor for PythonProcessor<F> {
    fn generate_prologue(&self, dependencies: &[String], task: &Task) -> Result<String> {
        let env_section = self.generate_python_env_section(task)?;
        let input_section = self.generate_python_input_section(dependencies);
        let param_section = self.generate_python_param_section(task)?;

        let prologue = format!(
            r#"# Otto-generated python prologue
import json
import os
import glob
import sys

# Set Otto environment variables
os.environ['OTTO_TASK_DIR'] = os.path.dirname(__file__)

# Import Otto builtins
import importlib.util
builtins_path = os.path.join(os.path.dirname(__file__), 'builtins.py')
spec = importlib.util.spec_from_file_location("otto_builtins", builtins_path)
otto_builtins = importlib.util.module_from_spec(spec)
spec.loader.exec_module(otto_builtins)

# Make builtin functions available globally
otto_get_input = otto_builtins.otto_get_input
otto_set_output = otto_builtins.otto_set_output
otto_deserialize_input = otto_builtins.otto_deserialize_input
otto_serialize_output = otto_builtins.otto_serialize_output

OTTO_INPUT = {{}}
OTTO_OUTPUT = {{}}

{env_section}
{input_section}
{param_section}"#
        );
        Ok(prologue)
    }

    fn generate_epilogue(&self) -> Result<String> {
        let epilogue = format!(
            "\n# Output Serialization\n\
             ################################################################################\n\
             # Serialize OTTO_OUTPUT to output.<task>.json using builtins\n\
             otto_serialize_output({})\n",
            python_quote(&self.task_name)
        );
        Ok(epilogue)
    }

    fn get_interpreter(&self) -> &str {
        "python3"
    }

    fn get_file_extension(&self) -> &str {
        "py"
    }

    fn create_builtins(&self) -> Result<()> {
        let builtins_path = self.workspace.task_dir(&self.task_name).join("builtins.py");

        // Ensure task directory exists before writing builtins
        if let Some(parent) = builtins_path.parent() {
            self.workspace.fs().create_dir_all_sync(parent)?;
        }

        let builtins_content = r#""""Otto Python Builtins
Functions to handle input/output file serialization
"""

import json
import os
import sys

def otto_deserialize_input(task_name):
    """Deserialize input.<task-name>.json -> OTTO_INPUT"""
    import __main__

    task_dir = os.environ.get('OTTO_TASK_DIR', '.')
    input_file = os.path.join(task_dir, f'input.{task_name}.json')

    if os.path.exists(input_file):
        try:
            with open(input_file, 'r') as f:
                data = json.load(f)

            # Ensure OTTO_INPUT exists
            if not hasattr(__main__, 'OTTO_INPUT'):
                __main__.OTTO_INPUT = {}

            # Load all key-value pairs with task name prefix
            for key, value in data.items():
                __main__.OTTO_INPUT[f'{task_name}.{key}'] = value

        except (json.JSONDecodeError, IOError) as e:
            print(f'Error: Failed to deserialize input from {task_name}: {e}', file=sys.stderr)
            return False
    return True

def otto_serialize_output(task_name):
    """Serialize OTTO_OUTPUT -> output.<task-name>.json"""
    import __main__

    task_dir = os.environ.get('OTTO_TASK_DIR', '.')
    output_file = os.path.join(task_dir, f'output.{task_name}.json')
    temp_file = output_file + '.tmp'

    # Get OTTO_OUTPUT or empty dict
    otto_output = getattr(__main__, 'OTTO_OUTPUT', {})

    try:
        # Write to temp file first for atomic operation
        with open(temp_file, 'w') as f:
            json.dump(otto_output, f, indent=2)

        # Atomic move
        os.rename(temp_file, output_file)
        return True

    except (IOError, OSError) as e:
        print(f'Error: Failed to serialize output to {output_file}: {e}', file=sys.stderr)
        # Clean up temp file if it exists
        if os.path.exists(temp_file):
            try:
                os.remove(temp_file)
            except OSError:
                pass
        return False

# Legacy helper functions for backward compatibility
def otto_get_input(key, default=None):
    """Safely get input value"""
    import __main__
    return getattr(__main__, 'OTTO_INPUT', {}).get(key, default)

def otto_set_output(key, value):
    """Set output value"""
    import __main__
    if not hasattr(__main__, 'OTTO_OUTPUT'):
        __main__.OTTO_OUTPUT = {}
    __main__.OTTO_OUTPUT[key] = value
"#;
        self.workspace
            .fs()
            .write_sync(&builtins_path, builtins_content.as_bytes())?;
        Ok(())
    }
}

#[path = "action_tests.rs"]
mod tests;
