use eyre::{Result, eyre};
use std::collections::HashMap;
use std::env;
use std::process::Command;

/// Evaluate environment variables with shell command substitution and variable resolution.
///
/// `base_overrides` LAYERS under the declared map: it is applied on top of the
/// inherited process environment before anything is evaluated, and it is the
/// floor of the returned map. That is what makes `otto.envs-command` output
/// visible to task bodies while a literal `envs:` entry for the same key still
/// wins the final value - and, because `evaluation_context` seeds a key's
/// *inherited* value when evaluating that key, a shadowing literal that
/// self-references (`FOO: '$(echo "${FOO:-x}")'`) sees the computed value
/// rather than the OS one. Merging the overrides into the declared map instead
/// would discard the computed value on exactly that shape.
///
/// Callers with nothing to layer pass an empty map and behave identically to
/// before the parameter existed.
pub fn evaluate_envs(
    envs: &HashMap<String, String>,
    working_dir: Option<&std::path::Path>,
    base_overrides: &HashMap<String, String>,
) -> Result<HashMap<String, String>> {
    log::debug!(
        "cfg::evaluate_envs: keys={} working_dir={working_dir:?} base_overrides={}",
        envs.len(),
        base_overrides.len()
    );
    let mut evaluated = HashMap::new();
    // Name *and* value, borrowed straight from `envs`: carrying the pair is what
    // keeps the retry loop free of a map lookup that could only be unwrapped.
    // Sorted, not HashMap order: the pass a key lands in decides how many passes
    // the whole map needs, so an unsorted seed made both the pass count and the
    // reported cycle vary run to run on identical input.
    let mut pending: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    pending.sort_unstable_by_key(|(k, _)| *k);
    let mut iterations = 0;
    // One pass per key, plus one. Every pass either resolves at least one key,
    // is the single retry of the parked keys below, or hits the no-progress
    // branch, so a map of N keys cannot need more than N + 1 passes and this
    // bound is unreachable for any resolvable input.
    //
    // It was a flat 100, which made a valid 200-deep chain a coin flip: measured
    // over 20 runs of one unchanged ottofile, 9 resolved correctly and 11 exited 1
    // with "Maximum iterations reached", purely on HashMap seeding order.
    let max_iterations = envs.len() + 1;
    // Keys whose evaluation failed while other declared keys were still
    // unresolved. They get exactly one more run once everything else has
    // settled; see the `Err` arm in the loop.
    let mut parked: Vec<(&str, &str, eyre::Report)> = Vec::new();

    // The environment otto itself was invoked with, before any declared key shadows it,
    // with `base_overrides` layered on top. This is what a declared key's own expression
    // reads when it references itself.
    let mut inherited: HashMap<String, String> = env::vars().collect();
    inherited.extend(base_overrides.iter().map(|(k, v)| (k.clone(), v.clone())));

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
            // Deferral is decided on the RAW value, before anything runs. A `$(...)`
            // body can reference a declared sibling, and `referenced_vars` scans the
            // body under the same rules as the expander, so the reference is visible
            // here. It was not visible before: the substitution executed first, with
            // the sibling stripped from the context, and spliced the empty read in as
            // the answer. `A: '$(echo "got:$B")'` with `B: hello` resolved to `got:`
            // whenever A was reached first, and to `got:hello` when B was.
            //
            // Only a DECLARED key defers. A reference to an inherited name resolves
            // from the inherited environment inside the command, which is the
            // `$(echo "${SOME_PROFILE:-default}")` idiom, and must not wait for
            // anything. A key referencing its own name is excluded too: that reads
            // the inherited seed (see [`evaluation_context`]), so deferring on it
            // would never clear.
            if let Some(unresolved) = referenced_vars(raw_value)
                .into_iter()
                .find(|name| name.as_str() != var_name && envs.contains_key(name) && !evaluated.contains_key(name))
            {
                log::debug!("cfg::evaluate_envs: deferring '{var_name}' on unresolved '{unresolved}'");
                still_pending.push((var_name, raw_value));
                continue;
            }

            let context = evaluation_context(&base_env, &evaluated, &inherited, var_name);

            match evaluate_single_env_value(raw_value, &context, working_dir) {
                Ok(resolved_value) => {
                    evaluated.insert(var_name.to_string(), resolved_value);
                    made_progress = true;
                }
                // Every declared reference this value SPELLS is resolved, or the
                // check above would have deferred it. But a command body can read
                // a sibling without naming it - `$(env | grep '^Z_HOST=')`, or a
                // tool that reads `VAULT_ADDR` from its environment - and that
                // sibling is stripped from the context until it resolves. So a
                // failure here is parked, not returned: once every other key has
                // settled, the command runs one more time against the complete
                // environment, and only a failure then is terminal. Returning at
                // once, which is what the first cut of 2.3.0 did, failed at load
                // a v2.2.1 ottofile whose `$(env | grep '^Z_HOST=' ...)` key
                // sorted before `Z_HOST`. Two runs of a failing command at most,
                // not one per pass, and a key that fails with nothing else
                // pending is not retried at all.
                Err(e) => {
                    log::debug!("cfg::evaluate_envs: parking '{var_name}' after a failed evaluation: {e}");
                    parked.push((var_name, raw_value, e));
                }
            }
        }

        // Every other declared key is resolved: the parked keys get their one
        // retry. If this pass WAS that retry, nothing else ran, `made_progress`
        // is false, and the failure stands.
        if still_pending.is_empty() && !parked.is_empty() {
            if !made_progress {
                let (var_name, _, e) = parked.swap_remove(0);
                return Err(eyre!("Failed to resolve environment variable '{}': {}", var_name, e));
            }
            pending = parked.drain(..).map(|(name, raw_value, _)| (name, raw_value)).collect();
            continue;
        }

        if !made_progress && !still_pending.is_empty() {
            // A key still deferred here may be waiting on a parked one, so the
            // parked failure is the root cause and is reported first; a cycle
            // report would blame the waiting end.
            if let Some((var_name, _, e)) = parked.first() {
                return Err(eyre!("Failed to resolve environment variable '{}': {}", var_name, e));
            }

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

    // The overrides are the floor, the declared map is the ceiling: an explicit
    // `envs:` entry wins its key, everything else the layer carried survives.
    let mut result = base_overrides.clone();
    result.extend(evaluated);
    Ok(result)
}

/// Parse `KEY=VALUE` lines (the `otto.envs-command` output format) into a map.
///
/// Deliberately narrow, because command output is DATA: values are taken
/// literally, with no unquoting, no `$(...)` re-evaluation and no `${VAR}`
/// expansion. The line format is `env` and `export`'s, so duplicate keys are
/// last-wins rather than an error.
///
/// Skipped: blank lines, and lines whose first non-space character is `#`.
/// Loud, naming the line number and the line: a line with no `=`, and a key
/// that is not `[A-Za-z_][A-Za-z0-9_]*` (which is what makes `FOO = bar`, whose
/// key would be `FOO `, an error rather than a silently-misnamed variable).
/// Empty input is legal and means "no variables" - an env set can legitimately
/// be empty, unlike a `choices-command`'s validation set.
pub fn parse_env_assignments(stdout: &str) -> Result<HashMap<String, String>> {
    let mut parsed = HashMap::new();
    for (index, raw_line) in stdout.lines().enumerate() {
        let number = index + 1;
        // `str::lines` already drops a `\r\n`'s carriage return; this catches
        // the lone trailing `\r` a Windows-ish generator can still emit, which
        // would otherwise ride into the value invisibly.
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            return Err(eyre!("line {number} is not KEY=VALUE: '{line}'"));
        };
        if !crate::naming::is_identifier(key) {
            return Err(eyre!("line {number} has an invalid key '{key}': '{line}'"));
        }
        parsed.insert(key.to_string(), value.to_string());
    }
    Ok(parsed)
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
    // own environment (the context this resolves against carries the inherited
    // environment, which is what made the leak reachable at all), and output
    // containing an undefined `$word` failed the
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

    // What the child actually gets: a cleared environment, then the seven names
    // below out of otto's own environment, then `env_overrides` on top of that.
    // `env_overrides` is the evaluation context, which is the whole inherited
    // environment minus the declared keys, plus the declared values resolved so
    // far. So this is not an isolated environment and the clear is not what keeps
    // otto's environment out of the command - the command sees nearly all of it,
    // by design, because that is how `$(echo "${SOME_PROFILE:-default}")` reads
    // the value otto was invoked with.
    //
    // The essential list matters in exactly one case: when one of those seven
    // names is itself a declared key. The context has it stripped until it
    // resolves, so without this floor a command would run with, say, no PATH.
    cmd.env_clear();

    let essential_vars = ["PATH", "HOME", "USER", "SHELL", "TERM", "LANG", "LC_ALL"];
    for var in &essential_vars {
        if let Ok(value) = env::var(var) {
            cmd.env(var, value);
        }
    }

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
    walk_var_refs(input, |_, reference| lookup(reference))
}

/// The variable a brace reference reads: its leading identifier, after an
/// optional `#` (`${#NAME}` is a length expansion). `${NAME}` gives `NAME`,
/// `${NAME:-x}` and `${NAME%y}` give `NAME`, `${#NAME}` gives `NAME`. A
/// reference with no leading identifier (`${}`, `${1}`) is returned whole, so it
/// fails lookup naming exactly what was written.
fn expansion_name(reference: &str) -> &str {
    let body = reference.strip_prefix('#').unwrap_or(reference);
    let end = body
        .bytes()
        .position(|b| !(b.is_ascii_alphanumeric() || b == b'_'))
        .unwrap_or(body.len());
    if end == 0 || body.as_bytes()[0].is_ascii_digit() {
        return reference;
    }
    &body[..end]
}

/// The one scan behind [`expand_var_refs`] and [`referenced_vars`]. For every
/// reference, `on_ref` receives the variable NAME the shell would read and the
/// raw REFERENCE otto resolves. They are the same text for `$NAME` and `${NAME}`
/// and differ for a parameter expansion: `${NAME:-x}` names `NAME` and
/// references `NAME:-x`. otto's templates do not implement parameter expansion,
/// so the expander looks the raw reference up and fails naming it (pinned by
/// `tests/dollar_escape_test.rs`); inside a `$(...)` body the shell does
/// implement it, so the deferral scan has to see `NAME`, or `AAA: '$(echo
/// "${ZZZ:-x}")'` runs before `ZZZ` resolves and prints the fallback. `on_ref`
/// returning `None` is an unresolved reference.
fn walk_var_refs<F>(input: &str, mut on_ref: F) -> Result<String>
where
    F: FnMut(&str, &str) -> Option<String>,
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
                let reference = &input[i + 2..close];
                let value = on_ref(expansion_name(reference), reference)
                    .ok_or_else(|| eyre!("Environment variable '{}' not found", reference))?;
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
                let value = on_ref(name, name).ok_or_else(|| eyre!("Environment variable '{}' not found", name))?;
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

/// Every variable name `value` references, in order: the name the shell would
/// read, so `${NAME:-x}` reports `NAME`. Same scan as the expander
/// ([`walk_var_refs`]), so the two cannot disagree about where a reference is.
fn referenced_vars(value: &str) -> Vec<String> {
    let mut names = Vec::new();
    let _ = walk_var_refs(value, |name, _| {
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
            //
            // A key's own name is not such a reference, for the same reason the
            // deferral rule excludes it (`name.as_str() != var_name` in
            // `evaluate_envs`): `AAA: '$AAA $BBB'` reads the inherited seed for
            // `$AAA`, by design. Following it anyway reported the legal
            // self-reference as `AAA -> AAA` and never reached the real cycle
            // further down the chain.
            let Some(next) = referenced_vars(value)
                .into_iter()
                .find(|name| name.as_str() != current && pending_set.contains(name.as_str()))
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
