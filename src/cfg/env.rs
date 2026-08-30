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
    // Name *and* value, borrowed straight from `envs`: carrying the pair is what
    // keeps the retry loop free of a map lookup that could only be unwrapped.
    // Sorted, not HashMap order: the pass a key lands in decides how many passes
    // the whole map needs, so an unsorted seed made both the pass count and the
    // reported cycle vary run to run on identical input.
    let mut pending: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    pending.sort_unstable_by_key(|(k, _)| *k);
    let mut iterations = 0;
    // One pass per key, plus one. Every pass either resolves at least one key or
    // hits the no-progress branch below, so a map of N keys cannot need more than
    // N passes and this bound is unreachable for any resolvable input.
    //
    // It was a flat 100, which made a valid 200-deep chain a coin flip: measured
    // over 20 runs of one unchanged ottofile, 9 resolved correctly and 11 exited 1
    // with "Maximum iterations reached", purely on HashMap seeding order.
    let max_iterations = envs.len() + 1;

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

    while !pending.is_empty() && iterations < max_iterations {
        iterations += 1;
        let mut made_progress = false;
        let mut still_pending = Vec::new();

        for (var_name, raw_value) in std::mem::take(&mut pending) {
            let context = evaluation_context(&base_env, &evaluated, &inherited, var_name);

            match evaluate_single_env_value(raw_value, &context, working_dir) {
                Ok(resolved_value) => {
                    evaluated.insert(var_name.to_string(), resolved_value);
                    made_progress = true;
                }
                Err(_) => {
                    // Might depend on other variables not yet resolved
                    still_pending.push((var_name, raw_value));
                }
            }
        }

        if !made_progress && !still_pending.is_empty() {
            // A cycle is its own error, named as one. Checked before the retry
            // below, whose message ("... 'B' not found") describes a variable
            // that exists and blames the wrong end of the loop.
            let still_pending_names: Vec<String> = still_pending.iter().map(|(name, _)| (*name).to_string()).collect();
            if let Some(cycle) = find_reference_cycle(&still_pending_names, envs) {
                return Err(eyre!(
                    "Circular dependency between environment variables: {}",
                    cycle.join(" -> ")
                ));
            }

            // Try to evaluate remaining variables with partial resolution
            for (var_name, raw_value) in &still_pending {
                let context = evaluation_context(&base_env, &evaluated, &inherited, var_name);
                match evaluate_single_env_value(raw_value, &context, working_dir) {
                    Ok(resolved_value) => {
                        evaluated.insert((*var_name).to_string(), resolved_value);
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

    if iterations >= max_iterations && !pending.is_empty() {
        // Unreachable for any resolvable map: the no-progress branch above owns
        // cycles and unresolvable references, and it names them. Kept as a
        // fail-closed backstop rather than dropping the resolved-so-far map.
        return Err(eyre!(
            "Maximum iterations ({}) reached while resolving {} environment variables - possible circular dependency",
            max_iterations,
            envs.len()
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

#[path = "env_tests.rs"]
mod tests;
