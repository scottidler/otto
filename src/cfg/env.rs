use eyre::{Result, eyre};
use regex::Regex;
use std::collections::HashMap;
use std::env;
use std::process::Command;

/// Evaluate environment variables with shell command substitution and variable resolution
pub fn evaluate_envs(
    envs: &HashMap<String, String>,
    working_dir: Option<&std::path::Path>,
) -> Result<HashMap<String, String>> {
    let mut evaluated = HashMap::new();
    let mut pending: Vec<String> = envs.keys().cloned().collect();
    let mut iterations = 0;
    const MAX_ITERATIONS: usize = 100; // Prevent infinite loops

    // The environment otto itself was invoked with, before any declared key shadows it.
    // This is what a declared key's own expression reads when it references itself.
    let inherited: HashMap<String, String> = env::vars().collect();

    // Baseline context shared by every expression: system environment minus every declared
    // key, so a reference to a declared-but-not-yet-resolved key defers instead of silently
    // reading the inherited value.
    let mut base_env = inherited.clone();
    for key in envs.keys() {
        base_env.remove(key);
    }

    while !pending.is_empty() && iterations < MAX_ITERATIONS {
        iterations += 1;
        let mut made_progress = false;
        let mut still_pending = Vec::new();

        for var_name in pending {
            let raw_value = envs.get(&var_name).unwrap();
            let context = evaluation_context(&base_env, &evaluated, &inherited, &var_name);

            match evaluate_single_env_value(raw_value, &context, working_dir) {
                Ok(resolved_value) => {
                    evaluated.insert(var_name, resolved_value);
                    made_progress = true;
                }
                Err(_) => {
                    // Might depend on other variables not yet resolved
                    still_pending.push(var_name);
                }
            }
        }

        if !made_progress && !still_pending.is_empty() {
            // Try to evaluate remaining variables with partial resolution
            for var_name in &still_pending {
                let raw_value = envs.get(var_name).unwrap();
                let context = evaluation_context(&base_env, &evaluated, &inherited, var_name);
                match evaluate_single_env_value(raw_value, &context, working_dir) {
                    Ok(resolved_value) => {
                        evaluated.insert(var_name.clone(), resolved_value);
                    }
                    Err(e) => {
                        return Err(eyre!("Failed to resolve environment variable '{}': {}", var_name, e));
                    }
                }
            }
            break;
        }

        pending = still_pending;
    }

    if iterations >= MAX_ITERATIONS {
        return Err(eyre!(
            "Maximum iterations reached while resolving environment variables - possible circular dependency"
        ));
    }

    Ok(evaluated)
}

/// Build the evaluation context for one expression.
///
/// It is (inherited environment minus all declared keys) + declared values resolved so far +
/// the inherited value of the key currently being evaluated. Seeding the inherited value for
/// that one key is what lets `MYVAR: '$(echo "${MYVAR:-fallback}")'` read the value otto was
/// invoked with. Seeding it for every declared key would let a cross-reference to a
/// not-yet-resolved key resolve to the inherited value instead of deferring, and which one
/// happened would depend on HashMap iteration order.
fn evaluation_context(
    base_env: &HashMap<String, String>,
    resolved: &HashMap<String, String>,
    inherited: &HashMap<String, String>,
    var_name: &str,
) -> HashMap<String, String> {
    let mut context = base_env.clone();
    if let Some(inherited_value) = inherited.get(var_name) {
        context.insert(var_name.to_string(), inherited_value.clone());
    }
    // Resolved declared values win over the inherited seed.
    context.extend(resolved.iter().map(|(k, v)| (k.clone(), v.clone())));
    context
}

/// Evaluate a single environment variable value with shell command substitution and variable resolution
fn evaluate_single_env_value(
    value: &str,
    env_context: &HashMap<String, String>,
    working_dir: Option<&std::path::Path>,
) -> Result<String> {
    let mut result = value.to_string();

    // Step 1: Resolve shell command substitution $(...)
    // Pass env_context to prevent parent environment pollution
    result = resolve_shell_commands_with_env(&result, working_dir, env_context)?;

    result = resolve_env_variables(&result, env_context)?;

    Ok(result)
}

/// Resolve shell command substitution patterns with explicit environment
fn resolve_shell_commands_with_env(
    input: &str,
    working_dir: Option<&std::path::Path>,
    env_context: &HashMap<String, String>,
) -> Result<String> {
    let re = Regex::new(r"\$\(([^)]+)\)").unwrap();
    let mut result = input.to_string();

    for captures in re.captures_iter(input) {
        let full_match = &captures[0];
        let command_str = &captures[1];

        // Execute the shell command with controlled environment
        let output = execute_shell_command_with_env(command_str, working_dir, env_context)?;
        result = result.replace(full_match, &output);
    }

    Ok(result)
}

fn execute_shell_command_with_env(
    command_str: &str,
    working_dir: Option<&std::path::Path>,
    env_overrides: &HashMap<String, String>,
) -> Result<String> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command_str);

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    // ALWAYS use controlled environment to prevent parent process pollution
    // This is critical for preventing Otto's environment from leaking into subprocesses
    cmd.env_clear();

    let essential_vars = ["PATH", "HOME", "USER", "SHELL", "TERM", "LANG", "LC_ALL"];
    for var in &essential_vars {
        if let Ok(value) = env::var(var) {
            cmd.env(var, value);
        }
    }

    // Add the explicit environment context (variables we're building up during evaluation)
    cmd.envs(env_overrides);

    let output = cmd
        .output()
        .map_err(|e| eyre!("Failed to execute command '{}': {}", command_str, e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!(
            "Command '{}' failed with exit code {}: {}",
            command_str,
            output.status.code().unwrap_or(-1),
            stderr
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.trim().to_string())
}

/// Resolve environment variable references: ${VAR} and $VAR
fn resolve_env_variables(input: &str, env_context: &HashMap<String, String>) -> Result<String> {
    let mut result = input.to_string();

    let re_braced = Regex::new(r"\$\{([^}]+)\}").unwrap();
    for captures in re_braced.captures_iter(input) {
        let full_match = &captures[0];
        let var_name = &captures[1];

        // ONLY use env_context - never fall back to system environment
        // This ensures proper test isolation and prevents environment pollution
        let var_value = env_context
            .get(var_name)
            .ok_or_else(|| eyre!("Environment variable '{}' not found", var_name))?;

        result = result.replace(full_match, var_value);
    }

    // Handle $VAR pattern (less specific, handle after braced)
    let re_simple = Regex::new(r"\$([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    for captures in re_simple.captures_iter(&result.clone()) {
        let full_match = &captures[0];
        let var_name = &captures[1];

        // Skip if this is part of a ${...} pattern we already handled
        if input.contains(&format!("${{{var_name}}}")) {
            continue;
        }

        // ONLY use env_context - never fall back to system environment
        // This ensures proper test isolation and prevents environment pollution
        let var_value = env_context
            .get(var_name)
            .ok_or_else(|| eyre!("Environment variable '{}' not found", var_name))?;

        result = result.replace(full_match, var_value);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::collections::HashMap;

    #[test]
    fn test_resolve_env_variables() {
        let mut env_context = HashMap::new();
        env_context.insert("USER".to_string(), "testuser".to_string());
        env_context.insert("VERSION".to_string(), "1.0.0".to_string());

        let result = resolve_env_variables("Hello ${USER}, version is $VERSION", &env_context).unwrap();
        assert_eq!(result, "Hello testuser, version is 1.0.0");
    }

    #[test]
    fn test_resolve_shell_commands() {
        let result = resolve_shell_commands_with_env("Today is $(date +%Y-%m-%d)", None, &HashMap::new()).unwrap();
        assert!(result.starts_with("Today is 20")); // Should be a date like "Today is 2024-01-15"
    }

    #[test]
    #[serial]
    fn test_evaluate_envs_simple() {
        let mut envs = HashMap::new();
        envs.insert("GREETING".to_string(), "Hello ${TEST_USER}".to_string());

        unsafe {
            env::set_var("TEST_USER", "testuser");
        }

        let result = evaluate_envs(&envs, None).unwrap();
        assert_eq!(result.get("GREETING").unwrap(), "Hello testuser");

        // Clean up our test variable
        unsafe {
            env::remove_var("TEST_USER");
        }
    }

    #[test]
    fn test_evaluate_envs_with_shell_command() {
        let mut envs = HashMap::new();
        envs.insert("ECHO_TEST".to_string(), "$(echo hello world)".to_string());

        let result = evaluate_envs(&envs, None).unwrap();
        assert_eq!(result.get("ECHO_TEST").unwrap(), "hello world");
    }

    #[test]
    fn test_evaluate_envs_dependency_chain() {
        let mut envs = HashMap::new();
        envs.insert("BASE".to_string(), "myapp".to_string());
        envs.insert("VERSION".to_string(), "$(echo 1.0.0)".to_string());
        envs.insert("FULL_NAME".to_string(), "${BASE}-${VERSION}".to_string());

        let result = evaluate_envs(&envs, None).unwrap();
        assert_eq!(result.get("BASE").unwrap(), "myapp");
        assert_eq!(result.get("VERSION").unwrap(), "1.0.0");
        assert_eq!(result.get("FULL_NAME").unwrap(), "myapp-1.0.0");
    }

    /// The motivating idiom: declare a default under the variable's own name and let the
    /// invoking shell override it. The self-reference must reach the `$()` subprocess.
    #[test]
    #[serial]
    fn test_evaluate_envs_self_reference_reads_inherited_value() {
        let key = "OTTO_TEST_SELF_INHERITED";
        unsafe {
            env::set_var(key, "from-shell");
        }

        let mut envs = HashMap::new();
        envs.insert(key.to_string(), format!("$(echo \"${{{key}:-fallback}}\")"));

        let result = evaluate_envs(&envs, None).unwrap();

        unsafe {
            env::remove_var(key);
        }
        assert_eq!(result.get(key).unwrap(), "from-shell");
    }

    /// Same expression with nothing inherited still takes the declared default.
    #[test]
    #[serial]
    fn test_evaluate_envs_self_reference_falls_back_when_unset() {
        let key = "OTTO_TEST_SELF_UNSET";
        unsafe {
            env::remove_var(key);
        }

        let mut envs = HashMap::new();
        envs.insert(key.to_string(), format!("$(echo \"${{{key}:-fallback}}\")"));

        let result = evaluate_envs(&envs, None).unwrap();
        assert_eq!(result.get(key).unwrap(), "fallback");
    }

    /// Self-reference outside `$()` resolves against the inherited value too.
    #[test]
    #[serial]
    fn test_evaluate_envs_self_reference_outside_command_substitution() {
        let key = "OTTO_TEST_SELF_BRACED";
        unsafe {
            env::set_var(key, "/base");
        }

        let mut envs = HashMap::new();
        envs.insert(key.to_string(), format!("${{{key}}}/suffix"));

        let result = evaluate_envs(&envs, None).unwrap();

        unsafe {
            env::remove_var(key);
        }
        assert_eq!(result.get(key).unwrap(), "/base/suffix");
    }

    /// All insertion orders of `n` items, so the test drives adversarial orderings rather
    /// than whichever one HashMap happens to hand back.
    fn permutations(n: usize) -> Vec<Vec<usize>> {
        if n == 0 {
            return vec![Vec::new()];
        }
        let mut out = Vec::new();
        for smaller in permutations(n - 1) {
            for position in 0..=smaller.len() {
                let mut order = smaller.clone();
                order.insert(position, n - 1);
                out.push(order);
            }
        }
        out
    }

    /// Property: a reference to another declared key always resolves to that key's DECLARED
    /// value, never to its inherited one, no matter what order the map is built or iterated
    /// in. Every declared key below also exists in the inherited environment, so reading the
    /// inherited value anywhere shows up in the chain.
    #[test]
    #[serial]
    fn test_evaluate_envs_cross_references_never_read_inherited_values() {
        let keys = [
            "OTTO_TEST_ORDER_A",
            "OTTO_TEST_ORDER_B",
            "OTTO_TEST_ORDER_C",
            "OTTO_TEST_ORDER_D",
        ];
        unsafe {
            env::set_var(keys[0], "inherited-a");
            env::set_var(keys[1], "INHERITED_B");
            env::set_var(keys[2], "INHERITED_C");
            env::set_var(keys[3], "INHERITED_D");
        }

        // A is a self-reference (must read inherited-a); B, C, D are cross-references and
        // must chain off the DECLARED values, including A's post-self-reference value.
        let declared = [
            (keys[0], format!("${{{}}}-x", keys[0])),
            (keys[1], format!("${{{}}}-b", keys[0])),
            (keys[2], format!("${{{}}}-c", keys[1])),
            (keys[3], format!("${{{}}}-d", keys[2])),
        ];

        let mut failures = Vec::new();
        for order in permutations(declared.len()) {
            // Repeat each insertion order: a fresh HashMap gets a fresh hasher, so repeats
            // sample different iteration orders for the same insertion order.
            for _ in 0..8 {
                let mut envs = HashMap::new();
                for &index in &order {
                    let (key, value) = &declared[index];
                    envs.insert((*key).to_string(), value.clone());
                }

                let result = evaluate_envs(&envs, None).unwrap();
                let expected = [
                    (keys[0], "inherited-a-x"),
                    (keys[1], "inherited-a-x-b"),
                    (keys[2], "inherited-a-x-b-c"),
                    (keys[3], "inherited-a-x-b-c-d"),
                ];
                for (key, want) in expected {
                    let got = result.get(key).unwrap();
                    if got != want {
                        failures.push(format!("order {order:?}: {key} = {got:?}, want {want:?}"));
                    }
                }
            }
        }

        unsafe {
            for key in keys {
                env::remove_var(key);
            }
        }
        assert!(
            failures.is_empty(),
            "cross-reference resolved wrong:\n{}",
            failures.join("\n")
        );
    }

    /// Two keys defined in terms of each other still fail loudly instead of silently
    /// resolving to inherited values or spinning forever.
    #[test]
    #[serial]
    fn test_evaluate_envs_circular_reference_still_errors() {
        let (a, b) = ("OTTO_TEST_CIRCULAR_A", "OTTO_TEST_CIRCULAR_B");
        unsafe {
            env::remove_var(a);
            env::remove_var(b);
        }

        let mut envs = HashMap::new();
        envs.insert(a.to_string(), format!("${{{b}}}"));
        envs.insert(b.to_string(), format!("${{{a}}}"));

        let error = evaluate_envs(&envs, None).unwrap_err().to_string();
        assert!(
            error.contains("Failed to resolve environment variable") && error.contains("not found"),
            "expected a loud circular-reference failure, got: {error}"
        );
    }

    /// A circular pair whose keys ARE inherited must still error: the inherited value is
    /// seeded only for the key under evaluation, never for the key it points at.
    #[test]
    #[serial]
    fn test_evaluate_envs_circular_reference_errors_even_when_inherited() {
        let (a, b) = ("OTTO_TEST_CIRCULAR_INH_A", "OTTO_TEST_CIRCULAR_INH_B");
        unsafe {
            env::set_var(a, "inherited-a");
            env::set_var(b, "inherited-b");
        }

        let mut envs = HashMap::new();
        envs.insert(a.to_string(), format!("${{{b}}}"));
        envs.insert(b.to_string(), format!("${{{a}}}"));

        let result = evaluate_envs(&envs, None);

        unsafe {
            env::remove_var(a);
            env::remove_var(b);
        }
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("Failed to resolve environment variable") && error.contains("not found"),
            "expected a loud circular-reference failure, got: {error}"
        );
    }
}
