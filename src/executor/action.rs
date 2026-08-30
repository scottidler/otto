use crate::ports::FileSystem;
use eyre::Result;
use hex;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

use super::task::Task;
use super::workspace::Workspace;

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
            let path = self.write_script(&processor, &script)?;
            let hash = self.calculate_hash(&script)?;
            Ok(ProcessedAction::Bash { path, script, hash })
        } else if trimmed_action.starts_with("#!/usr/bin/env python3")
            || trimmed_action.starts_with("#!/usr/bin/python3")
        {
            let processor = PythonProcessor::new(self.workspace.clone(), &self.task_name);
            processor.create_builtins()?;
            let script = self.build_script(&processor, user_action, task)?;
            let path = self.write_script(&processor, &script)?;
            let hash = self.calculate_hash(&script)?;
            Ok(ProcessedAction::Python3 { path, script, hash })
        } else {
            // Default to bash if no shebang is detected (for backward compatibility)
            let processor = BashProcessor::new(self.workspace.clone(), &self.task_name);
            processor.create_builtins()?;
            let script = self.build_script(&processor, user_action, task)?;
            let path = self.write_script(&processor, &script)?;
            let hash = self.calculate_hash(&script)?;
            Ok(ProcessedAction::Bash { path, script, hash })
        }
    }

    fn build_script<T: ScriptProcessor>(&self, processor: &T, user_action: &str, task: &Task) -> Result<String> {
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

    fn write_script<T: ScriptProcessor>(&self, processor: &T, script: &str) -> Result<PathBuf> {
        // Calculate hash for caching
        let hash = self.calculate_hash(script)?;

        // Write to cache directory
        let cache_file = self
            .workspace
            .cache_dir()
            .join(format!("{}.{}", hash, processor.get_file_extension()));

        // Ensure cache directory exists
        self.workspace.fs().create_dir_all_sync(self.workspace.cache_dir())?;

        // Write script to cache if it doesn't exist
        if !self.workspace.fs().exists_sync(&cache_file) {
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

        Ok(script_path)
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

    fn generate_bash_env_section(&self, task: &Task) -> String {
        let yaml_envs = self.get_yaml_env_vars(task);
        if yaml_envs.is_empty() {
            return String::new();
        }

        let mut env_exports = vec![
            "# Environment Variables".to_string(),
            "################################################################################".to_string(),
        ];

        // Export only actual environment variables from YAML (not CLI parameters)
        for (key, value) in &yaml_envs {
            // Allow shell expansion by properly quoting the value and preserving case
            env_exports.push(format!("export {key}=\"{value}\""));
        }

        env_exports.push(String::new()); // Add blank line after section
        env_exports.join("\n")
    }

    fn generate_bash_input_section(&self, dependencies: &[String]) -> String {
        if dependencies.is_empty() {
            return String::new();
        }

        let mut input_section = vec![
            "# Input Loading".to_string(),
            "################################################################################".to_string(),
        ];

        // Use builtins functions for deserialization
        for dep in dependencies {
            input_section.push(format!("otto_deserialize_input \"{dep}\""));
        }

        input_section.push(String::new()); // Add blank line after section
        input_section.join("\n")
    }

    fn generate_bash_param_section(&self, task: &Task) -> String {
        if task.values.is_empty() {
            return String::new();
        }

        let mut param_section = vec![
            "# Parameter Assignments".to_string(),
            "################################################################################".to_string(),
        ];

        // Simple parameter assignments for CLI parameters only
        for (param_name, param_value) in &task.values {
            let value_str = match param_value {
                crate::cfg::param::Value::Item(s) => s.clone(),
                crate::cfg::param::Value::List(l) => l.join(" "),
                crate::cfg::param::Value::Dict(d) => {
                    // Convert dict to space-separated key=value pairs
                    d.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")
                }
                crate::cfg::param::Value::Empty => String::new(),
            };
            // Convert hyphens to underscores for valid bash variable names
            let bash_var_name = param_name.replace('-', "_");
            param_section.push(format!("{bash_var_name}=\"{value_str}\""));
        }

        param_section.push(String::new()); // Add blank line after section
        param_section.join("\n")
    }
}

impl<F: FileSystem> ScriptProcessor for BashProcessor<F> {
    fn generate_prologue(&self, dependencies: &[String], task: &Task) -> Result<String> {
        let env_section = self.generate_bash_env_section(task);
        let input_section = self.generate_bash_input_section(dependencies);
        let param_section = self.generate_bash_param_section(task);

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
            r#"
# Output Serialization
################################################################################
# Serialize OTTO_OUTPUT to output.{}.json using builtins
otto_serialize_output "{}"
"#,
            self.task_name, self.task_name
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

    fn generate_python_env_section(&self, task: &Task) -> String {
        let yaml_envs = self.get_yaml_env_vars(task);
        if yaml_envs.is_empty() {
            return String::new();
        }

        let mut env_section = vec![
            "# Environment Variables".to_string(),
            "################################################################################".to_string(),
        ];

        // Set only actual environment variables from YAML (not CLI parameters)
        for (key, value) in &yaml_envs {
            // Allow shell expansion by evaluating the value
            env_section.push(format!("os.environ['{}'] = '{}'", key.to_uppercase(), value));
        }

        env_section.push(String::new()); // Add blank line after section
        env_section.join("\n")
    }

    fn generate_python_input_section(&self, dependencies: &[String]) -> String {
        if dependencies.is_empty() {
            return String::new();
        }

        let mut input_section = vec![
            "# Input Loading".to_string(),
            "################################################################################".to_string(),
        ];

        // Use builtins functions for deserialization
        for dep in dependencies {
            input_section.push(format!("otto_deserialize_input(\"{dep}\")"));
        }

        input_section.push(String::new()); // Add blank line after section
        input_section.join("\n")
    }

    fn generate_python_param_section(&self, task: &Task) -> String {
        if task.values.is_empty() {
            return String::new();
        }

        let mut param_section = vec![
            "# Parameter Assignments".to_string(),
            "################################################################################".to_string(),
        ];

        // Simple parameter assignments for CLI parameters only
        for (param_name, param_value) in &task.values {
            let value_str = match param_value {
                crate::cfg::param::Value::Item(s) => s.clone(),
                crate::cfg::param::Value::List(l) => l.join(" "),
                crate::cfg::param::Value::Dict(d) => {
                    // Convert dict to space-separated key=value pairs
                    d.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")
                }
                crate::cfg::param::Value::Empty => String::new(),
            };
            param_section.push(format!("{param_name} = '{value_str}'"));
        }

        param_section.push(String::new()); // Add blank line after section
        param_section.join("\n")
    }
}

impl<F: FileSystem> ScriptProcessor for PythonProcessor<F> {
    fn generate_prologue(&self, dependencies: &[String], task: &Task) -> Result<String> {
        let env_section = self.generate_python_env_section(task);
        let input_section = self.generate_python_input_section(dependencies);
        let param_section = self.generate_python_param_section(task);

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
            r#"
# Output Serialization
################################################################################
# Serialize OTTO_OUTPUT to output.{}.json using builtins
otto_serialize_output("{}")
"#,
            self.task_name, self.task_name
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::param::Value;
    use hex;
    use serial_test::serial;
    use sha2::Digest;
    use std::collections::HashMap;
    use tempfile::TempDir;

    /// Helper to set up a test-specific database path and OTTO_HOME
    fn setup_test_db(temp_dir: &std::path::Path) {
        let db_path = temp_dir.join("test_otto.db");
        let otto_home = temp_dir.join(".otto");
        // SAFETY: This is safe in tests because we control the execution environment
        // and tests are isolated. The env var is set before any StateManager is created.
        unsafe {
            std::env::set_var("OTTO_DB_PATH", &db_path);
            std::env::set_var("OTTO_HOME", &otto_home);
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_bash_action_processing() -> Result<()> {
        let temp_dir = TempDir::new()?;
        setup_test_db(temp_dir.path());
        let workspace = Arc::new(Workspace::new(temp_dir.path().to_path_buf()).await?);
        workspace.init().await?;

        let processor = ActionProcessor::new(workspace.clone(), "test_task")?;

        let mut task_envs = HashMap::new();
        task_envs.insert("greeting".to_string(), "hello".to_string());

        let mut task_values = HashMap::new();
        task_values.insert("greeting".to_string(), Value::Item("hello".to_string()));

        let task = Task::new(
            "test_task".to_string(),
            None,
            vec![crate::executor::task::TaskEdge::success("dep_task")],
            vec![],
            vec![],
            task_envs,
            task_values,
            "#!/usr/bin/env bash\necho \"${greeting} world\"".to_string(),
        );

        // Process the action
        let result = processor.process(&task.action, &task)?;

        match result {
            ProcessedAction::Bash { path, script, hash } => {
                assert!(path.exists());
                assert!(script.contains("declare -a OTTO_INPUT"));
                assert!(script.contains("declare -a OTTO_OUTPUT"));
                assert!(script.contains("export OTTO_TASK_DIR"));
                assert!(script.contains("greeting=\"hello\""));
                assert!(script.contains("otto_deserialize_input \"dep_task\""));
                assert!(script.contains("echo \"${greeting} world\""));
                assert!(script.contains("otto_serialize_output \"test_task\""));

                assert_eq!(hash.len(), 8, "Hash should be 8 characters");
                assert!(
                    hash.chars().all(|c| c.is_ascii_hexdigit()),
                    "Hash should be hexadecimal"
                );

                let mut hasher = sha2::Sha256::new();
                hasher.update(script.as_bytes());
                let expected_hash = hex::encode(hasher.finalize())[..8].to_string();
                assert_eq!(hash, expected_hash, "Hash should match generated script content");
            }
            _ => panic!("Expected Bash variant"),
        }

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_python_action_processing() -> Result<()> {
        let temp_dir = TempDir::new()?;
        setup_test_db(temp_dir.path());
        let workspace = Arc::new(Workspace::new(temp_dir.path().to_path_buf()).await?);
        workspace.init().await?;

        let processor = ActionProcessor::new(workspace.clone(), "test_task")?;

        let mut task_envs = HashMap::new();
        task_envs.insert("name".to_string(), "world".to_string());

        let mut task_values = HashMap::new();
        task_values.insert("name".to_string(), Value::Item("world".to_string()));

        let task = Task::new(
            "test_task".to_string(),
            None,
            vec![crate::executor::task::TaskEdge::success("dep_task")],
            vec![],
            vec![],
            task_envs,
            task_values,
            "#!/usr/bin/env python3\nprint(f\"Hello {name}\")".to_string(),
        );

        // Process the action
        let result = processor.process(&task.action, &task)?;

        match result {
            ProcessedAction::Python3 { path, script, hash } => {
                assert!(path.exists());
                assert!(script.contains("OTTO_INPUT = {}"));
                assert!(script.contains("OTTO_OUTPUT = {}"));
                assert!(script.contains("os.environ['OTTO_TASK_DIR']"));
                assert!(script.contains("name = 'world'"));
                assert!(script.contains("otto_deserialize_input(\"dep_task\")"));
                assert!(script.contains("print(f\"Hello {name}\")"));
                assert!(script.contains("otto_serialize_output(\"test_task\")"));

                assert_eq!(hash.len(), 8, "Hash should be 8 characters");
                assert!(
                    hash.chars().all(|c| c.is_ascii_hexdigit()),
                    "Hash should be hexadecimal"
                );

                let mut hasher = sha2::Sha256::new();
                hasher.update(script.as_bytes());
                let expected_hash = hex::encode(hasher.finalize())[..8].to_string();
                assert_eq!(hash, expected_hash, "Hash should match generated script content");
            }
            _ => panic!("Expected Python3 variant"),
        }

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_default_bash_action_processing() -> Result<()> {
        let temp_dir = TempDir::new()?;
        setup_test_db(temp_dir.path());
        let workspace = Arc::new(Workspace::new(temp_dir.path().to_path_buf()).await?);
        workspace.init().await?;

        let processor = ActionProcessor::new(workspace.clone(), "test_task")?;

        let mut task_envs = HashMap::new();
        task_envs.insert("message".to_string(), "hello".to_string());

        let mut task_values = HashMap::new();
        task_values.insert("message".to_string(), Value::Item("hello".to_string()));

        let task = Task::new(
            "test_task".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            task_envs,
            task_values,
            "echo \"${message} from default bash\"".to_string(), // No shebang
        );

        // Process the action
        let result = processor.process(&task.action, &task)?;

        match result {
            ProcessedAction::Bash { path, script, hash } => {
                assert!(path.exists());
                assert!(script.contains("declare -a OTTO_INPUT"));
                assert!(script.contains("declare -a OTTO_OUTPUT"));
                assert!(script.contains("export OTTO_TASK_DIR"));
                assert!(script.contains("message=\"hello\""));
                assert!(script.contains("echo \"${message} from default bash\""));
                assert!(script.contains("otto_serialize_output \"test_task\""));

                assert_eq!(hash.len(), 8, "Hash should be 8 characters");
                assert!(
                    hash.chars().all(|c| c.is_ascii_hexdigit()),
                    "Hash should be hexadecimal"
                );

                let mut hasher = sha2::Sha256::new();
                hasher.update(script.as_bytes());
                let expected_hash = hex::encode(hasher.finalize())[..8].to_string();
                assert_eq!(hash, expected_hash, "Hash should match generated script content");
            }
            _ => panic!("Expected Bash variant (default fallback)"),
        }

        Ok(())
    }
}
