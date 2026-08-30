use eyre::{Result, eyre};
use std::collections::HashMap;
use std::env;
use std::process::Command;

/// Evaluate environment variables with shell command substitution and variable resolution
pub fn evaluate_envs(
    envs: &HashMap<String, String>,
    working_dir: Option<&std::path::Path>,
) -> Result<HashMap<String, String>> {
    log::debug!("cfg::evaluate_envs: keys={} working_dir={working_dir:?}", envs.len());
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

    // Structural check first: an unmatched `$(` is a config error, not a dependency that
    // might resolve on a later pass. Checking every raw value up front keeps it out of the
    // retry loop, where it would otherwise masquerade as "waiting on another variable".
    for (key, value) in envs {
        if let Err(e) = validate_command_substitutions(value) {
            return Err(eyre!("Environment variable '{}': {} in value '{}'", key, e, value));
        }
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
            // A cycle is its own error, named as one. Checked before the retry
            // below, whose message ("... 'B' not found") describes a variable
            // that exists and blames the wrong end of the loop.
            if let Some(cycle) = find_reference_cycle(&still_pending, envs) {
                return Err(eyre!(
                    "Circular dependency between environment variables: {}",
                    cycle.join(" -> ")
                ));
            }

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

/// Marker byte fencing a command-output placeholder. Not a character any shell
/// or YAML author writes, and - the point - it contains no `$`, so variable
/// resolution walks straight past it.
const PLACEHOLDER_MARK: char = '\u{1}';

/// Evaluate a single environment variable value with shell command substitution and variable resolution
fn evaluate_single_env_value(
    value: &str,
    env_context: &HashMap<String, String>,
    working_dir: Option<&std::path::Path>,
) -> Result<String> {
    // Step 1: run every `$(...)`, leaving a placeholder where its output goes.
    //
    // The output is data. Splicing it in before step 2 made step 2 read it as
    // syntax: `LEAK: "$(echo '$PARENTVAL')"` resolved `$PARENTVAL` out of otto's
    // own environment (a parent-env leak the controlled command environment
    // exists to prevent), and output containing an undefined `$word` failed the
    // whole config load. Placeholders keep the two stages from seeing each
    // other's text.
    let (templated, outputs) = resolve_shell_commands_with_env(value, working_dir, env_context)?;

    // Step 2: resolve ${VAR} / $VAR against the declared context only.
    let resolved = resolve_env_variables(&templated, env_context)?;

    // Step 3: put the command output back, verbatim.
    Ok(restore_command_outputs(&resolved, &outputs))
}

/// Resolve shell command substitution patterns with explicit environment.
///
/// Returns the value with each `$(...)` replaced by a placeholder, plus the
/// outputs in the order they were produced.
fn resolve_shell_commands_with_env(
    input: &str,
    working_dir: Option<&std::path::Path>,
    env_context: &HashMap<String, String>,
) -> Result<(String, Vec<String>)> {
    let mut result = String::with_capacity(input.len());
    let mut outputs: Vec<String> = Vec::new();
    let mut rest = input;

    while let Some((start, end)) = find_command_substitution(rest)? {
        result.push_str(&rest[..start]);

        // Strip the `$(` and the matching `)`; what's between them is the command.
        let command_str = &rest[start + 2..end - 1];

        // Execute the shell command with controlled environment
        let output = execute_shell_command_with_env(command_str, working_dir, env_context)?;
        result.push(PLACEHOLDER_MARK);
        result.push_str(&outputs.len().to_string());
        result.push(PLACEHOLDER_MARK);
        outputs.push(output);
        rest = &rest[end..];
    }

    result.push_str(rest);
    Ok((result, outputs))
}

/// Replace each placeholder with its command's output, in one pass so an output
/// that happens to contain a placeholder is never re-substituted.
fn restore_command_outputs(input: &str, outputs: &[String]) -> String {
    if outputs.is_empty() {
        return input.to_string();
    }

    let mut result = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(open) = rest.find(PLACEHOLDER_MARK) {
        let after_open = &rest[open + PLACEHOLDER_MARK.len_utf8()..];
        let Some(close) = after_open.find(PLACEHOLDER_MARK) else {
            break;
        };
        let index: Option<usize> = after_open[..close].parse().ok();
        match index.and_then(|i| outputs.get(i)) {
            Some(output) => {
                result.push_str(&rest[..open]);
                result.push_str(output);
                rest = &after_open[close + PLACEHOLDER_MARK.len_utf8()..];
            }
            None => {
                // Not one of ours: emit the mark and keep looking.
                result.push_str(&rest[..open + PLACEHOLDER_MARK.len_utf8()]);
                rest = after_open;
            }
        }
    }

    result.push_str(rest);
    result
}

/// Which quoting context a byte inside a command substitution sits in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Quote {
    None,
    Single,
    Double,
}

/// Locate the first `$(...)` in `input`, returning `(start, end)` byte offsets where `start`
/// is the `$` and `end` is one past the MATCHING `)`.
///
/// The old `\$\(([^)]+)\)` regex stopped at the first `)` it saw, so `$(echo ")")` and any
/// nested `$( ... $( ... ) ... )` were truncated mid-command. This walks the substitution
/// instead, tracking what sh itself tracks when it looks for the closing paren:
///
/// - `(` opens a nesting level (subshell or a nested `$(`), `)` closes one
/// - inside single quotes nothing is special except the closing `'` (not even a backslash)
/// - inside double quotes a backslash escapes the next byte and `$(` still opens a nested
///   substitution whose quoting starts fresh, which is why the level stack holds a quote
///   state per level rather than one state overall
///
/// Scope note: only the region between `$(` and its `)` is quote-aware. The search for the
/// opening `$(` scans the raw value, matching the previous behavior (the value is a YAML
/// scalar, not shell input, so there is no outer quoting context to honor).
///
/// An unmatched `$(` is an error, never a silent literal pass-through.
fn find_command_substitution(input: &str) -> Result<Option<(usize, usize)>> {
    let bytes = input.as_bytes();
    let Some(start) = find_substitution_start(bytes) else {
        return Ok(None);
    };

    // One quote state per open level; the outer `$(` is the first.
    let mut levels = vec![Quote::None];
    let mut escaped = false;
    let mut i = start + 2;

    while i < bytes.len() {
        let byte = bytes[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }

        let quote = *levels.last().expect("level stack is never empty while scanning");
        let opens_substitution = byte == b'$' && bytes.get(i + 1) == Some(&b'(');
        match quote {
            Quote::Single => {
                if byte == b'\'' {
                    *levels.last_mut().unwrap() = Quote::None;
                }
            }
            // A nested `$(` starts a fresh quoting context even inside double quotes, which
            // is why each level carries its own state.
            Quote::Double if opens_substitution => {
                levels.push(Quote::None);
                i += 2;
                continue;
            }
            Quote::Double => match byte {
                b'\\' => escaped = true,
                b'"' => *levels.last_mut().unwrap() = Quote::None,
                _ => {}
            },
            Quote::None if opens_substitution => {
                levels.push(Quote::None);
                i += 2;
                continue;
            }
            Quote::None => match byte {
                b'\\' => escaped = true,
                b'\'' => *levels.last_mut().unwrap() = Quote::Single,
                b'"' => *levels.last_mut().unwrap() = Quote::Double,
                b'(' => levels.push(Quote::None),
                b')' => {
                    levels.pop();
                    if levels.is_empty() {
                        return Ok(Some((start, i + 1)));
                    }
                }
                _ => {}
            },
        }

        i += 1;
    }

    Err(eyre!("unmatched '$(' (no closing ')')"))
}

/// Byte offset of the first *unescaped* `$(` in `bytes`.
///
/// `$$` is the escape for a literal `$` (see `resolve_env_variables`), so it is
/// consumed here too: `$$(echo hi)` is the four literal characters `$(ec`... -
/// text, not a command. Honoring the escape in only one of the two stages would
/// mean there is no way to write a literal `$(` at all.
fn find_substitution_start(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'$' {
            match bytes[i + 1] {
                b'$' => {
                    i += 2;
                    continue;
                }
                b'(' => return Some(i),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Walk every `$(...)` in a value so an unmatched `$(` anywhere is caught, not just a
/// leading one.
fn validate_command_substitutions(input: &str) -> Result<()> {
    let mut rest = input;
    while let Some((_, end)) = find_command_substitution(rest)? {
        rest = &rest[end..];
    }
    Ok(())
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

/// Resolve environment variable references: `${VAR}`, `$VAR`, and `$$` for a
/// literal `$`.
///
/// One left-to-right pass, and substituted text is never rescanned. The old
/// implementation ran two regexes and `String::replace`d each match across the
/// whole value, which had three consequences: every occurrence of a reference was
/// replaced by whichever match was seen first; a value that resolved to something
/// containing `$FOO` had that expanded too (data read as syntax); and a guard was
/// needed to stop the `$VAR` pass from re-touching what the `${VAR}` pass had
/// already replaced. That guard keyed off the *name*, so `"a=${FOO} b=$FOO"` left
/// the bare `$FOO` unresolved in the generated script - visible only there,
/// because a terminal `echo` re-expanded the raw template and printed the value
/// anyway.
fn resolve_env_variables(input: &str, env_context: &HashMap<String, String>) -> Result<String> {
    expand_var_refs(input, |name| env_context.get(name).cloned())
}

/// Walk `input` resolving `${NAME}` / `$NAME` through `lookup`, treating `$$` as
/// an escaped literal `$`. `lookup` returning `None` is an unresolved reference.
pub(crate) fn expand_var_refs<F>(input: &str, mut lookup: F) -> Result<String>
where
    F: FnMut(&str) -> Option<String>,
{
    let bytes = input.as_bytes();
    let mut result = String::with_capacity(input.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'$' {
            let start = i;
            while i < bytes.len() && bytes[i] != b'$' {
                i += 1;
            }
            result.push_str(&input[start..i]);
            continue;
        }

        match bytes.get(i + 1) {
            // `$$` - the escape for a literal dollar sign. Without it there is no
            // way to put a `$` in an env value that bash will not later read as a
            // PID or a variable.
            Some(b'$') => {
                result.push('$');
                i += 2;
            }
            Some(b'{') => {
                let Some(close) = input[i + 2..].find('}').map(|offset| i + 2 + offset) else {
                    // Unterminated `${`: literal text, same as the old regex.
                    result.push('$');
                    i += 1;
                    continue;
                };
                let name = &input[i + 2..close];
                let value = lookup(name).ok_or_else(|| eyre!("Environment variable '{}' not found", name))?;
                result.push_str(&value);
                i = close + 1;
            }
            Some(&c) if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i + 1;
                let mut end = start;
                while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                    end += 1;
                }
                let name = &input[start..end];
                let value = lookup(name).ok_or_else(|| eyre!("Environment variable '{}' not found", name))?;
                result.push_str(&value);
                i = end;
            }
            // A `$` before anything else (including end of string) is literal.
            _ => {
                result.push('$');
                i += 1;
            }
        }
    }

    Ok(result)
}

/// Every variable name `value` references, in order. Same scanning rules as the
/// expander, so "what it references" and "what it resolves" cannot disagree.
fn referenced_vars(value: &str) -> Vec<String> {
    let mut names = Vec::new();
    let _ = expand_var_refs(value, |name| {
        names.push(name.to_string());
        Some(String::new())
    });
    names
}

/// Find a reference cycle among `pending`, returned as the path that closes it.
///
/// Without this a real cycle (`A: $B`, `B: $A`) reported `Failed to resolve
/// environment variable 'A': Environment variable 'B' not found`, which names a
/// missing variable that is not missing and points at the wrong one of the two.
fn find_reference_cycle(pending: &[String], envs: &HashMap<String, String>) -> Option<Vec<String>> {
    let pending_set: std::collections::HashSet<&str> = pending.iter().map(String::as_str).collect();

    for start in pending {
        let mut path: Vec<String> = vec![start.clone()];
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        seen.insert(start.clone());
        let mut current = start.clone();

        while let Some(value) = envs.get(&current) {
            // Follow the first reference that is itself still unresolved; a cycle
            // reachable this way is a cycle, which is all the message needs.
            let Some(next) = referenced_vars(value)
                .into_iter()
                .find(|name| pending_set.contains(name.as_str()))
            else {
                break;
            };
            path.push(next.clone());
            if next == *start {
                return Some(path);
            }
            if !seen.insert(next.clone()) {
                break;
            }
            current = next;
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::collections::HashMap;

    /// Substitute and immediately splice the outputs back in.
    ///
    /// Production keeps the two apart on purpose (command output must not be
    /// rescanned for variable references), but for the cases below - which are
    /// about *finding* the substitution, not about what happens after it - the
    /// spliced text is the thing under test.
    fn substitute(
        input: &str,
        working_dir: Option<&std::path::Path>,
        env_context: &HashMap<String, String>,
    ) -> Result<String> {
        let (templated, outputs) = resolve_shell_commands_with_env(input, working_dir, env_context)?;
        Ok(restore_command_outputs(&templated, &outputs))
    }

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
        let result = substitute("Today is $(date +%Y-%m-%d)", None, &HashMap::new()).unwrap();
        assert!(result.starts_with("Today is 20")); // Should be a date like "Today is 2024-01-15"
    }

    /// One adversarial `$(...)` case: the raw value, the command text the scanner must carve
    /// out of it, and what the whole value must resolve to.
    struct SubstitutionCase {
        name: &'static str,
        value: &'static str,
        command: &'static str,
        expected: &'static str,
        /// False when wrapping the value in double quotes would change what sh itself sees,
        /// so the differential check below skips it.
        sh_comparable: bool,
    }

    /// The adversarial table the design doc's risk row calls for ("scanner mismatches sh's
    /// own parse of an edge case"). Every case here truncated or blew up under the old
    /// `\$\(([^)]+)\)` regex except the plain and adjacent ones.
    fn substitution_cases() -> Vec<SubstitutionCase> {
        vec![
            SubstitutionCase {
                name: "plain",
                value: "$(echo hello)",
                command: "echo hello",
                expected: "hello",
                sh_comparable: true,
            },
            SubstitutionCase {
                name: "nested substitution",
                value: r#"$(echo "$(basename /a/b)")"#,
                command: r#"echo "$(basename /a/b)""#,
                expected: "b",
                sh_comparable: true,
            },
            SubstitutionCase {
                name: "nested substitution, unquoted",
                value: "$(echo $(echo deep))",
                command: "echo $(echo deep)",
                expected: "deep",
                sh_comparable: true,
            },
            SubstitutionCase {
                name: "close paren in double quotes",
                value: r#"$(echo ")")"#,
                command: r#"echo ")""#,
                expected: ")",
                sh_comparable: true,
            },
            SubstitutionCase {
                name: "close paren in single quotes",
                value: "$(echo 'a)b')",
                command: "echo 'a)b'",
                expected: "a)b",
                sh_comparable: true,
            },
            SubstitutionCase {
                name: "open paren in single quotes",
                value: "$(echo 'a(b')",
                command: "echo 'a(b'",
                expected: "a(b",
                sh_comparable: true,
            },
            SubstitutionCase {
                name: "backslash-escaped close paren",
                value: r"$(echo \))",
                command: r"echo \)",
                expected: ")",
                sh_comparable: true,
            },
            SubstitutionCase {
                name: "backslash-escaped quote inside double quotes",
                value: r#"$(echo "a\"b)c")"#,
                command: r#"echo "a\"b)c""#,
                expected: r#"a"b)c"#,
                sh_comparable: true,
            },
            SubstitutionCase {
                name: "backslash is literal inside single quotes",
                value: r"$(echo 'a\')",
                command: r"echo 'a\'",
                expected: r"a\",
                sh_comparable: true,
            },
            SubstitutionCase {
                name: "single quote inside double quotes",
                value: r#"$(echo "it's )")"#,
                command: r#"echo "it's )""#,
                expected: "it's )",
                sh_comparable: true,
            },
            SubstitutionCase {
                name: "double quote inside single quotes",
                value: r#"$(echo 'say ")')"#,
                command: r#"echo 'say ")'"#,
                expected: r#"say ")"#,
                sh_comparable: true,
            },
            SubstitutionCase {
                name: "subshell group",
                value: "$( (echo grouped) )",
                command: " (echo grouped) ",
                expected: "grouped",
                sh_comparable: true,
            },
            SubstitutionCase {
                name: "adjacent to literal text",
                value: "pre-$(echo mid)-post",
                command: "echo mid",
                expected: "pre-mid-post",
                sh_comparable: true,
            },
            SubstitutionCase {
                name: "empty substitution",
                value: "[$()]",
                command: "",
                expected: "[]",
                sh_comparable: true,
            },
        ]
    }

    /// The scanner carves out exactly the command sh would see, and the value resolves to
    /// what sh would produce.
    #[test]
    fn test_command_substitution_adversarial_table() {
        let mut failures = Vec::new();
        for case in substitution_cases() {
            match find_command_substitution(case.value) {
                Ok(Some((start, end))) => {
                    let found = &case.value[start + 2..end - 1];
                    if found != case.command {
                        failures.push(format!(
                            "{}: boundary carved {found:?}, want {:?}",
                            case.name, case.command
                        ));
                    }
                }
                other => failures.push(format!("{}: expected a substitution, got {other:?}", case.name)),
            }

            match substitute(case.value, None, &HashMap::new()) {
                Ok(resolved) if resolved == case.expected => {}
                Ok(resolved) => failures.push(format!(
                    "{}: resolved {resolved:?}, want {:?}",
                    case.name, case.expected
                )),
                Err(e) => failures.push(format!("{}: resolution failed: {e}", case.name)),
            }
        }
        assert!(
            failures.is_empty(),
            "adversarial cases failed:\n{}",
            failures.join("\n")
        );
    }

    /// Differential check against sh itself: the scanner's job is to agree with the shell
    /// about where a substitution ends, so ask the shell.
    #[test]
    fn test_command_substitution_agrees_with_sh() {
        let mut failures = Vec::new();
        for case in substitution_cases().into_iter().filter(|c| c.sh_comparable) {
            let script = format!("printf '%s' \"{}\"", case.value);
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(&script)
                .output()
                .expect("failed to run sh");
            let sh_says = String::from_utf8_lossy(&output.stdout).to_string();
            if sh_says != case.expected {
                failures.push(format!(
                    "{}: sh produced {sh_says:?} for {script:?}, table says {:?}",
                    case.name, case.expected
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "table expectations disagree with sh:\n{}",
            failures.join("\n")
        );
    }

    /// Several substitutions in one value each resolve independently, in order.
    #[test]
    fn test_multiple_substitutions_in_one_value() {
        let result = substitute(r#"$(echo one)/$(echo "t)wo")/$(echo three)"#, None, &HashMap::new()).unwrap();
        assert_eq!(result, "one/t)wo/three");
    }

    /// Repeats of the same substitution each execute and each get their own output, instead
    /// of one output being pasted over every occurrence.
    #[test]
    fn test_repeated_substitution_resolves_each_occurrence() {
        let result = substitute("$(echo x)-$(echo x)", None, &HashMap::new()).unwrap();
        assert_eq!(result, "x-x");
    }

    /// An unmatched `$(` is a loud error, never a literal pass-through.
    #[test]
    fn test_unmatched_substitution_is_an_error() {
        for value in [
            "$(echo hello",
            r#"$(echo ")"#,
            "$(echo '",
            "$(echo $(echo inner)",
            "ok $(echo done) then $(echo oops",
        ] {
            let error = substitute(value, None, &HashMap::new()).unwrap_err().to_string();
            assert!(
                error.contains("unmatched '$('"),
                "expected an unmatched-paren error for {value:?}, got: {error}"
            );
        }
    }

    /// The unmatched-paren config error names the offending key and its value, and fires
    /// before anything is evaluated.
    #[test]
    fn test_evaluate_envs_unmatched_substitution_names_key_and_value() {
        let mut envs = HashMap::new();
        envs.insert("BROKEN".to_string(), "$(echo hello".to_string());

        let error = evaluate_envs(&envs, None).unwrap_err().to_string();
        assert!(
            error.contains("BROKEN") && error.contains("unmatched '$('") && error.contains("$(echo hello"),
            "expected key and value in the error, got: {error}"
        );
    }

    /// Nesting and quoting resolve through the full `evaluate_envs` path, not just the
    /// scanner, and a sibling key in the same map is unaffected.
    #[test]
    fn test_evaluate_envs_nested_and_quoted_substitutions() {
        let mut envs = HashMap::new();
        envs.insert("NESTED".to_string(), r#"$(echo "$(basename /a/b)")"#.to_string());
        envs.insert("PARENQ".to_string(), r#"$(echo ")")"#.to_string());
        envs.insert("SIBLING".to_string(), "plain-value".to_string());

        let result = evaluate_envs(&envs, None).unwrap();
        assert_eq!(result.get("NESTED").unwrap(), "b");
        assert_eq!(result.get("PARENQ").unwrap(), ")");
        assert_eq!(result.get("SIBLING").unwrap(), "plain-value");
    }

    /// A value with no substitution at all comes through untouched.
    #[test]
    fn test_value_without_substitution_is_unchanged() {
        let result = substitute("no substitutions (but parens) here", None, &HashMap::new()).unwrap();
        assert_eq!(result, "no substitutions (but parens) here");
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
    /// resolving to inherited values or spinning forever - and the message says
    /// *circular*. It used to say `Failed to resolve environment variable 'A':
    /// Environment variable 'B' not found`, which names a variable that is not
    /// missing and blames whichever end of the loop HashMap order reached first.
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
            error.contains("Circular dependency between environment variables")
                && error.contains(a)
                && error.contains(b),
            "expected a cycle named as a cycle, got: {error}"
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
            error.contains("Circular dependency between environment variables")
                && error.contains(a)
                && error.contains(b),
            "expected a cycle named as a cycle, got: {error}"
        );
    }

    /// Command output is data. It used to be spliced in before variable
    /// resolution, so a `$NAME` inside the output was resolved - out of otto's
    /// own environment, which is the leak the controlled command environment
    /// exists to prevent.
    #[test]
    #[serial]
    fn command_output_is_not_rescanned_for_variable_references() {
        unsafe {
            env::set_var("OTTO_TEST_PARENTVAL", "leaked-parent-value");
        }

        let mut envs = HashMap::new();
        envs.insert("LEAK".to_string(), "$(echo '$OTTO_TEST_PARENTVAL')".to_string());
        let result = evaluate_envs(&envs, None);

        unsafe {
            env::remove_var("OTTO_TEST_PARENTVAL");
        }

        let evaluated = result.expect("command output must not fail the load");
        assert_eq!(
            evaluated.get("LEAK").map(String::as_str),
            Some("$OTTO_TEST_PARENTVAL"),
            "output must arrive as the literal text the command printed"
        );
    }

    /// Same defect, the other symptom: output naming something undefined used to
    /// fail the whole config load rather than being carried as text.
    #[test]
    #[serial]
    fn command_output_naming_an_undefined_variable_is_still_text() {
        let mut envs = HashMap::new();
        envs.insert("MSG".to_string(), "$(echo 'cost is $undefined_thing')".to_string());

        let evaluated = evaluate_envs(&envs, None).expect("undefined name in output must not fail the load");
        assert_eq!(
            evaluated.get("MSG").map(String::as_str),
            Some("cost is $undefined_thing")
        );
    }

    /// `${FOO}` and `$FOO` in one value both resolve. The old guard skipped the
    /// bare form whenever the braced form appeared anywhere in the same value,
    /// and only the generated script showed it (`export BOTH="a=fooval b=$FOO"`).
    #[test]
    fn both_reference_forms_resolve_in_one_value() {
        let mut context = HashMap::new();
        context.insert("FOO".to_string(), "fooval".to_string());

        let result = resolve_env_variables("a=${FOO} b=$FOO", &context).unwrap();
        assert_eq!(result, "a=fooval b=fooval");
    }

    /// A resolved value is not itself rescanned, so a value that happens to
    /// contain `$SOMETHING` stays as it is.
    #[test]
    fn a_substituted_value_is_not_rescanned() {
        let mut context = HashMap::new();
        context.insert("FOO".to_string(), "$BAR".to_string());
        context.insert("BAR".to_string(), "should-not-appear".to_string());

        let result = resolve_env_variables("${FOO}", &context).unwrap();
        assert_eq!(result, "$BAR");
    }

    /// `$$` is the literal-dollar escape, and it is honored by both stages: the
    /// variable pass and the command-substitution scan.
    #[test]
    fn double_dollar_is_a_literal_dollar() {
        let context = HashMap::new();
        assert_eq!(resolve_env_variables("cost: $$5", &context).unwrap(), "cost: $5");

        let mut envs = HashMap::new();
        envs.insert("LITERAL".to_string(), "$$(echo ran)".to_string());
        let evaluated = evaluate_envs(&envs, None).expect("an escaped $( must not execute");
        assert_eq!(
            evaluated.get("LITERAL").map(String::as_str),
            Some("$(echo ran)"),
            "an escaped substitution must stay text"
        );
    }
}
