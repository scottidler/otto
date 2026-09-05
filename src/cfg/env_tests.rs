#![cfg(test)]

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

    let error = evaluate_envs(&envs, None, &HashMap::new()).unwrap_err().to_string();
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

    let result = evaluate_envs(&envs, None, &HashMap::new()).unwrap();
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

    let result = evaluate_envs(&envs, None, &HashMap::new()).unwrap();
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

    let result = evaluate_envs(&envs, None, &HashMap::new()).unwrap();
    assert_eq!(result.get("ECHO_TEST").unwrap(), "hello world");
}

#[test]
fn test_evaluate_envs_dependency_chain() {
    let mut envs = HashMap::new();
    envs.insert("BASE".to_string(), "myapp".to_string());
    envs.insert("VERSION".to_string(), "$(echo 1.0.0)".to_string());
    envs.insert("FULL_NAME".to_string(), "${BASE}-${VERSION}".to_string());

    let result = evaluate_envs(&envs, None, &HashMap::new()).unwrap();
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

    let result = evaluate_envs(&envs, None, &HashMap::new()).unwrap();

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

    let result = evaluate_envs(&envs, None, &HashMap::new()).unwrap();
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

    let result = evaluate_envs(&envs, None, &HashMap::new()).unwrap();

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

            let result = evaluate_envs(&envs, None, &HashMap::new()).unwrap();
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

    let error = evaluate_envs(&envs, None, &HashMap::new()).unwrap_err().to_string();
    assert!(
        error.contains("Circular dependency between environment variables") && error.contains(a) && error.contains(b),
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

    let result = evaluate_envs(&envs, None, &HashMap::new());

    unsafe {
        env::remove_var(a);
        env::remove_var(b);
    }
    let error = result.unwrap_err().to_string();
    assert!(
        error.contains("Circular dependency between environment variables") && error.contains(a) && error.contains(b),
        "expected a cycle named as a cycle, got: {error}"
    );
}

/// A key that reads its own inherited value is legal (that is why the deferral
/// rule excludes a key's own name), so it is not the cycle. The reporter
/// followed it anyway and named `AAA -> AAA` while the real cycle, `BBB` and
/// `CCC` pointing at each other, went unmentioned.
#[test]
#[serial]
fn a_legal_self_reference_is_not_reported_as_the_cycle() {
    let (a, b, c) = (
        "OTTO_TEST_SELFREF_AAA",
        "OTTO_TEST_SELFREF_BBB",
        "OTTO_TEST_SELFREF_CCC",
    );
    unsafe {
        env::set_var(a, "seeded-from-the-environment");
        env::remove_var(b);
        env::remove_var(c);
    }

    let mut envs = HashMap::new();
    envs.insert(a.to_string(), format!("${a} ${b}"));
    envs.insert(b.to_string(), format!("${c}"));
    envs.insert(c.to_string(), format!("${b}"));

    let result = evaluate_envs(&envs, None, &HashMap::new());

    unsafe {
        env::remove_var(a);
    }
    let error = result.unwrap_err().to_string();
    assert!(
        error.contains("Circular dependency between environment variables"),
        "expected a cycle, got: {error}"
    );
    assert!(
        error.contains(b) && error.contains(c),
        "the message must name the real cycle, got: {error}"
    );
    assert!(
        !error.contains(&format!("{a} -> {a}")),
        "a key reading its own inherited value is not a cycle, got: {error}"
    );
}

/// Command output is data. It used to be spliced in before variable
/// resolution, so a `$NAME` inside the output was resolved - out of otto's
/// own environment, which the evaluation context carries.
#[test]
#[serial]
fn command_output_is_not_rescanned_for_variable_references() {
    unsafe {
        env::set_var("OTTO_TEST_PARENTVAL", "leaked-parent-value");
    }

    let mut envs = HashMap::new();
    envs.insert("LEAK".to_string(), "$(echo '$OTTO_TEST_PARENTVAL')".to_string());
    let result = evaluate_envs(&envs, None, &HashMap::new());

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

    let evaluated =
        evaluate_envs(&envs, None, &HashMap::new()).expect("undefined name in output must not fail the load");
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
    let evaluated = evaluate_envs(&envs, None, &HashMap::new()).expect("an escaped $( must not execute");
    assert_eq!(
        evaluated.get("LITERAL").map(String::as_str),
        Some("$(echo ran)"),
        "an escaped substitution must stay text"
    );
}

/// A long chain resolves regardless of depth, and regardless of the order the
/// map happens to hand its keys over.
///
/// The pass budget was a flat 100 while the seed order came straight off a
/// HashMap, so how many passes a chain needed varied run to run: measured over
/// 20 runs of one unchanged 200-deep ottofile, 9 resolved and 11 exited 1 with
/// "Maximum iterations reached". The budget is now one pass per key, which is a
/// bound no resolvable map can exceed.
#[test]
fn a_deep_chain_resolves_at_any_depth() {
    for depth in [105usize, 200, 250, 400] {
        let mut envs = HashMap::new();
        envs.insert("V0".to_string(), "base".to_string());
        for i in 1..=depth {
            envs.insert(format!("V{i}"), format!("${{V{}}}-{i}", i - 1));
        }

        let evaluated = evaluate_envs(&envs, None, &HashMap::new())
            .unwrap_or_else(|e| panic!("a {depth}-deep chain must resolve, got: {e}"));

        let tail = evaluated
            .get(&format!("V{depth}"))
            .unwrap_or_else(|| panic!("V{depth} missing at depth {depth}"));
        assert!(
            tail.starts_with("base-1-2-3-"),
            "chain resolved wrong at {depth}: {tail}"
        );
        assert!(
            tail.ends_with(&format!("-{depth}")),
            "chain truncated at {depth}: {tail}"
        );
    }
}

/// Repeating one resolution gives the same answer every time. The failure this
/// pins was not a wrong answer, it was two different answers for one input.
#[test]
fn a_deep_chain_resolves_identically_across_repeats() {
    let mut envs = HashMap::new();
    envs.insert("V0".to_string(), "base".to_string());
    for i in 1..=200 {
        envs.insert(format!("V{i}"), format!("${{V{}}}-{i}", i - 1));
    }

    let first = evaluate_envs(&envs, None, &HashMap::new()).expect("first pass must resolve");
    for run in 2..=10 {
        let again = evaluate_envs(&envs, None, &HashMap::new())
            .unwrap_or_else(|e| panic!("run {run} of the same map must resolve, got: {e}"));
        assert_eq!(first, again, "run {run} disagreed with run 1 on identical input");
    }
}

/// Raising the budget must not make a real cycle resolvable: the no-progress
/// branch still owns cycles and still names them.
#[test]
fn a_cycle_still_fails_closed_with_the_per_key_budget() {
    let mut envs = HashMap::new();
    envs.insert("X".to_string(), "${Y}-x".to_string());
    envs.insert("Y".to_string(), "${X}-y".to_string());

    let err = evaluate_envs(&envs, None, &HashMap::new())
        .expect_err("a cycle must not resolve")
        .to_string();
    assert!(
        err.contains("Circular dependency between environment variables"),
        "a cycle must be reported as one, got: {err}"
    );
}

// ----------------------------------------------------------------------
// Deferral is decided on the raw value, before any `$(...)` runs.
// ----------------------------------------------------------------------

/// Count the marker files a `$(...)` body dropped, one per execution.
fn marker_count(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir)
        .expect("marker dir must be readable")
        .filter(|entry| {
            entry
                .as_ref()
                .expect("marker entry must be readable")
                .file_name()
                .to_string_lossy()
                .starts_with("mark.")
        })
        .count()
}

/// A `$(...)` body referencing a declared sibling waits for the sibling, in the
/// order that used to lose. `pending` is sorted by key, so `A` referencing `B`
/// is the case where the reference is reached first: the substitution ran with
/// `B` stripped from the context and `A` resolved to `got:`.
#[test]
fn a_command_referencing_a_later_sibling_waits_for_it() {
    let mut envs = HashMap::new();
    envs.insert(
        "OTTO_TEST_SIB_A".to_string(),
        "$(echo \"got:$OTTO_TEST_SIB_B\")".to_string(),
    );
    envs.insert("OTTO_TEST_SIB_B".to_string(), "hello".to_string());

    let result = evaluate_envs(&envs, None, &HashMap::new()).expect("a sibling reference must resolve");

    assert_eq!(result.get("OTTO_TEST_SIB_A").map(String::as_str), Some("got:hello"));
    assert_eq!(result.get("OTTO_TEST_SIB_B").map(String::as_str), Some("hello"));
}

/// The other declaration order, which resolved correctly by luck before: the
/// referenced key sorts first, so it is already resolved when the command runs.
/// Both orders must give the same answer.
#[test]
fn a_command_referencing_an_earlier_sibling_still_resolves() {
    let mut envs = HashMap::new();
    envs.insert("OTTO_TEST_SIB_A".to_string(), "hello".to_string());
    envs.insert(
        "OTTO_TEST_SIB_B".to_string(),
        "$(echo \"got:$OTTO_TEST_SIB_A\")".to_string(),
    );

    let result = evaluate_envs(&envs, None, &HashMap::new()).expect("a sibling reference must resolve");

    assert_eq!(result.get("OTTO_TEST_SIB_B").map(String::as_str), Some("got:hello"));
}

/// The deferral rule keys on DECLARED names only. A reference to an INHERITED
/// name inside `$(...)` resolves from the inherited environment, in the command,
/// on the first pass - this is `otto-dev`'s vault-passthrough shape
/// (`$(echo "${OTTO_DEV_AUTH_PROFILE:-...}")`), and it must not wait for
/// anything.
#[test]
#[serial]
fn a_command_referencing_an_inherited_name_resolves_from_the_environment() {
    let inherited = "OTTO_TEST_INHERITED_IN_CMD";
    unsafe {
        env::set_var(inherited, "from-shell");
    }

    let mut envs = HashMap::new();
    envs.insert(
        "OTTO_TEST_VAULT".to_string(),
        format!("$(echo \"${{{inherited}:-fallback}}\")"),
    );

    let result = evaluate_envs(&envs, None, &HashMap::new());

    unsafe {
        env::remove_var(inherited);
    }
    let evaluated = result.expect("an inherited reference must resolve");
    assert_eq!(evaluated.get("OTTO_TEST_VAULT").map(String::as_str), Some("from-shell"));
}

/// A key whose `$(...)` body names the key itself reads the inherited seed and
/// terminates. It must not defer on itself: the seed is the only value it will
/// ever get, so waiting for "itself, resolved" would spin to the no-progress
/// branch and report a cycle. The declared sibling in the same value is there to
/// prove the two rules coexist - self excluded, sibling deferred.
#[test]
#[serial]
fn a_self_reference_inside_a_command_reads_the_inherited_value_and_terminates() {
    let (self_key, sibling) = ("OTTO_TEST_A_SELF", "OTTO_TEST_Z_SIB");
    unsafe {
        env::set_var(self_key, "from-shell");
    }

    let mut envs = HashMap::new();
    envs.insert(self_key.to_string(), format!("$(echo \"${self_key}-${sibling}\")"));
    envs.insert(sibling.to_string(), "sib".to_string());

    let result = evaluate_envs(&envs, None, &HashMap::new());

    unsafe {
        env::remove_var(self_key);
    }
    let evaluated = result.expect("a self-reference must terminate");
    assert_eq!(evaluated.get(self_key).map(String::as_str), Some("from-shell-sib"));
}

/// Two keys referencing each other inside `$(...)` are a cycle and are reported
/// as one. Before the deferral check they were not even an error: both commands
/// ran with the other key stripped from the context, so both resolved to the
/// empty read and the load succeeded with two wrong values.
#[test]
fn two_keys_referencing_each_other_inside_commands_report_a_cycle() {
    let mut envs = HashMap::new();
    envs.insert(
        "OTTO_TEST_CMD_CYC_A".to_string(),
        "$(echo \"$OTTO_TEST_CMD_CYC_B\")".to_string(),
    );
    envs.insert(
        "OTTO_TEST_CMD_CYC_B".to_string(),
        "$(echo \"$OTTO_TEST_CMD_CYC_A\")".to_string(),
    );

    let error = evaluate_envs(&envs, None, &HashMap::new())
        .expect_err("a cycle through command bodies must not resolve")
        .to_string();

    assert!(
        error.contains("Circular dependency between environment variables")
            && error.contains("OTTO_TEST_CMD_CYC_A")
            && error.contains("OTTO_TEST_CMD_CYC_B"),
        "expected a cycle named as a cycle, got: {error}"
    );
}

/// A command that fails while a sibling is still unresolved runs once more after
/// that sibling settles, and then its failure is final: two executions for this
/// two-key map, whatever its size. The pre-2.3.0 loop re-ran it on every pass
/// plus once in the partial-resolution fallback (three here, one per key for a
/// larger map); the first cut of 2.3.0 ran it once and failed at load, which
/// broke commands that read a sibling from their environment (next test). `$$`
/// is the shell's PID, so each execution leaves its own marker.
#[test]
fn a_failing_command_runs_once_more_after_its_siblings_resolve() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut envs = HashMap::new();
    envs.insert(
        "OTTO_TEST_A_BAD".to_string(),
        format!("$(touch {}/mark.$$; false)", dir.path().display()),
    );
    envs.insert("OTTO_TEST_Z_OK".to_string(), "plain".to_string());

    let error = evaluate_envs(&envs, None, &HashMap::new())
        .expect_err("a failing command must fail the load")
        .to_string();

    assert!(
        error.contains("OTTO_TEST_A_BAD") && error.contains("failed with exit code 1"),
        "expected the failing command named, got: {error}"
    );
    assert_eq!(
        marker_count(dir.path()),
        2,
        "the failing command must run once, then once more against the complete environment"
    );
}

/// With nothing else pending there is no environment to complete, so a failing
/// command is not retried at all.
#[test]
fn a_failing_command_with_no_siblings_runs_exactly_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut envs = HashMap::new();
    envs.insert(
        "OTTO_TEST_A_BAD".to_string(),
        format!("$(touch {}/mark.$$; false)", dir.path().display()),
    );

    let error = evaluate_envs(&envs, None, &HashMap::new())
        .expect_err("a failing command must fail the load")
        .to_string();

    assert!(
        error.contains("OTTO_TEST_A_BAD") && error.contains("failed with exit code 1"),
        "expected the failing command named, got: {error}"
    );
    assert_eq!(
        marker_count(dir.path()),
        1,
        "a lone failing command must run exactly once"
    );
}

/// A command can read a declared sibling without spelling its name: `env | grep`,
/// or any tool that reads `VAULT_ADDR`-style settings from its environment. The
/// reference scan cannot see that, so the sibling is absent from the command's
/// environment on the first run and the command fails. v2.2.1 retried it after
/// the sibling resolved and the load succeeded; the first cut of 2.3.0 failed the
/// load on the first run. The retry is the behavior; this pins it.
#[test]
fn a_command_reading_a_sibling_from_its_environment_waits_for_it() {
    let mut envs = HashMap::new();
    envs.insert(
        "OTTO_TEST_A_ENDPOINT".to_string(),
        "$(env | grep '^OTTO_TEST_Z_HOST=' | cut -d= -f2 | grep .)".to_string(),
    );
    envs.insert("OTTO_TEST_Z_HOST".to_string(), "vault.example".to_string());

    let result = evaluate_envs(&envs, None, &HashMap::new()).expect("the value must resolve");

    assert_eq!(
        result.get("OTTO_TEST_A_ENDPOINT").map(String::as_str),
        Some("vault.example"),
        "the command must see its sibling once the sibling has resolved"
    );
}

/// A key deferred on a failing sibling is waiting on the failure, not on a
/// cycle, so the failure is what gets reported.
#[test]
fn a_key_waiting_on_a_failed_sibling_reports_the_failure_not_a_cycle() {
    let mut envs = HashMap::new();
    envs.insert("OTTO_TEST_A_BAD".to_string(), "$(false)".to_string());
    envs.insert("OTTO_TEST_B_WAITS".to_string(), "$OTTO_TEST_A_BAD".to_string());

    let error = evaluate_envs(&envs, None, &HashMap::new())
        .expect_err("a failing command must fail the load")
        .to_string();

    assert!(
        error.contains("OTTO_TEST_A_BAD") && error.contains("failed with exit code 1"),
        "expected the failing command named, got: {error}"
    );
    assert!(
        !error.contains("Circular"),
        "a wait on a failed sibling is not a cycle, got: {error}"
    );
}

/// A value that is a command plus a reference to a later sibling defers whole:
/// the command does not run until the sibling is resolved. It used to run on
/// every pass, because the `${LATER}` in the same value failed variable
/// resolution after the command had already executed.
#[test]
fn a_command_beside_a_later_reference_runs_exactly_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut envs = HashMap::new();
    envs.insert(
        "OTTO_TEST_A_MIXED".to_string(),
        format!(
            "$(touch {}/mark.$$; echo ran) ${{OTTO_TEST_Z_LATER}}",
            dir.path().display()
        ),
    );
    envs.insert("OTTO_TEST_Z_LATER".to_string(), "late".to_string());

    let result = evaluate_envs(&envs, None, &HashMap::new()).expect("the value must resolve");

    assert_eq!(result.get("OTTO_TEST_A_MIXED").map(String::as_str), Some("ran late"));
    assert_eq!(marker_count(dir.path()), 1, "the command must run exactly once");
}

// ----------------------------------------------------------------------
// `base_overrides`: the layer `otto.envs-command` output arrives on.
// ----------------------------------------------------------------------

/// The layer is the floor of the returned map, so a computed key with no
/// literal `envs:` counterpart still reaches the task.
#[test]
fn base_overrides_survive_into_the_result() {
    let mut base = HashMap::new();
    base.insert("COMPUTED".to_string(), "from-command".to_string());

    let result = evaluate_envs(&HashMap::new(), None, &base).unwrap();

    assert_eq!(result.get("COMPUTED").map(String::as_str), Some("from-command"));
}

/// An explicit `envs:` entry wins its own key. This is the precedence rule
/// stated in the design doc: the more specific declaration wins, and it gives
/// a consumer a one-line override without editing the command.
#[test]
fn a_declared_key_beats_the_same_key_in_the_base_layer() {
    let mut base = HashMap::new();
    base.insert("FOO".to_string(), "computed".to_string());
    let mut envs = HashMap::new();
    envs.insert("FOO".to_string(), "explicit".to_string());

    let result = evaluate_envs(&envs, None, &base).unwrap();

    assert_eq!(result.get("FOO").map(String::as_str), Some("explicit"));
}

/// The layering-not-merging case: a declared key that *self-references* reads
/// the computed value, because the base layer seeds the inherited environment
/// that `evaluation_context` hands the key's own expression. Merging the
/// command's output into the declared map instead would resolve this to the
/// fallback (or, when the OS has the variable, to the OS value).
#[test]
#[serial]
fn a_self_referencing_declared_key_reads_the_base_layer_value() {
    let mut base = HashMap::new();
    base.insert("FOO".to_string(), "computed".to_string());
    let mut envs = HashMap::new();
    envs.insert("FOO".to_string(), "$(echo \"${FOO:-fallback}\")".to_string());

    let result = evaluate_envs(&envs, None, &base).unwrap();

    assert_eq!(
        result.get("FOO").map(String::as_str),
        Some("computed"),
        "a shadowing self-reference must see the layered value, not the fallback"
    );
}

/// A declared key can read a *different* key the layer carries, because the
/// layer is part of the base environment every expression evaluates against.
#[test]
fn a_declared_key_can_reference_a_base_layer_key() {
    let mut base = HashMap::new();
    base.insert("ROOT".to_string(), "/srv/web".to_string());
    let mut envs = HashMap::new();
    envs.insert("BIN".to_string(), "${ROOT}/bin".to_string());

    let result = evaluate_envs(&envs, None, &base).unwrap();

    assert_eq!(result.get("BIN").map(String::as_str), Some("/srv/web/bin"));
}

// ----------------------------------------------------------------------
// `parse_env_assignments`: the `KEY=VALUE` format `otto.envs-command` emits.
// ----------------------------------------------------------------------

#[test]
fn parse_env_assignments_reads_plain_pairs() {
    let parsed = parse_env_assignments("FOO=bar\nBAZ=qux\n").unwrap();

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed.get("FOO").map(String::as_str), Some("bar"));
    assert_eq!(parsed.get("BAZ").map(String::as_str), Some("qux"));
}

/// Empty output is legal and means "no variables" - an env set can
/// legitimately be empty on a machine with nothing cloned, unlike a
/// `choices-command`'s validation set.
#[test]
fn parse_env_assignments_accepts_empty_output() {
    assert!(parse_env_assignments("").unwrap().is_empty());
    assert!(parse_env_assignments("\n\n").unwrap().is_empty());
}

/// Split on the FIRST `=`, so a value containing `=` survives whole.
#[test]
fn parse_env_assignments_splits_on_the_first_equals() {
    let parsed = parse_env_assignments("FLAGS=-Dx=1 -Dy=2\n").unwrap();

    assert_eq!(parsed.get("FLAGS").map(String::as_str), Some("-Dx=1 -Dy=2"));
}

/// Whitespace inside a value survives byte-for-byte. This is why the command
/// runs through `run_command_stdout` (raw) rather than `run_lines_command`
/// (per-line `str::trim`).
#[test]
fn parse_env_assignments_keeps_whitespace_in_a_value() {
    let parsed = parse_env_assignments("KEY=  spaced value  \n").unwrap();

    assert_eq!(parsed.get("KEY").map(String::as_str), Some("  spaced value  "));
}

/// An empty value is a real value, not a missing one.
#[test]
fn parse_env_assignments_accepts_an_empty_value() {
    let parsed = parse_env_assignments("EMPTY=\n").unwrap();

    assert_eq!(parsed.get("EMPTY").map(String::as_str), Some(""));
}

#[test]
fn parse_env_assignments_skips_blank_and_comment_lines() {
    let parsed = parse_env_assignments("# a header\n\n   # indented comment\nFOO=bar\n").unwrap();

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed.get("FOO").map(String::as_str), Some("bar"));
}

/// Leading whitespace before the KEY is skipped; a `#` inside a value is not
/// a comment, since only the line's first non-space character starts one.
#[test]
fn parse_env_assignments_skips_leading_whitespace_before_the_key() {
    let parsed = parse_env_assignments("   FOO=bar # not a comment\n").unwrap();

    assert_eq!(parsed.get("FOO").map(String::as_str), Some("bar # not a comment"));
}

/// CRLF: the carriage return never rides into the value.
#[test]
fn parse_env_assignments_strips_a_trailing_carriage_return() {
    let parsed = parse_env_assignments("FOO=bar\r\nBAZ=qux\r\n").unwrap();

    assert_eq!(parsed.get("FOO").map(String::as_str), Some("bar"));
    assert_eq!(parsed.get("BAZ").map(String::as_str), Some("qux"));
}

/// Duplicate keys are last-wins, matching `env` and `export`, not an error.
#[test]
fn parse_env_assignments_takes_the_last_duplicate() {
    let parsed = parse_env_assignments("FOO=first\nFOO=second\n").unwrap();

    assert_eq!(parsed.get("FOO").map(String::as_str), Some("second"));
}

#[test]
fn parse_env_assignments_rejects_a_line_with_no_equals() {
    let err = parse_env_assignments("FOO=bar\nnot-a-kv\n")
        .expect_err("a line with no `=` must fail")
        .to_string();

    assert!(err.contains("line 2"), "must name the line number, got: {err}");
    assert!(err.contains("not-a-kv"), "must quote the line, got: {err}");
}

#[test]
fn parse_env_assignments_rejects_an_invalid_key() {
    for (line, bad_key) in [("1FOO=bar", "1FOO"), ("FOO-BAR=x", "FOO-BAR"), ("FOO =bar", "FOO ")] {
        let err = parse_env_assignments(line).unwrap_err().to_string();
        assert!(err.contains("line 1"), "must name the line number, got: {err}");
        assert!(
            err.contains(bad_key),
            "must name the offending key '{bad_key}', got: {err}"
        );
    }
}

/// A value is DATA: no unquoting, no `$(...)` re-evaluation, no `${VAR}`
/// expansion. The v2.0.0 injection-containment rule, applied to this source.
#[test]
fn parse_env_assignments_takes_values_literally() {
    let parsed = parse_env_assignments("A=$(rm -rf /)\nB=${HOME}\nC=\"quoted\"\n").unwrap();

    assert_eq!(parsed.get("A").map(String::as_str), Some("$(rm -rf /)"));
    assert_eq!(parsed.get("B").map(String::as_str), Some("${HOME}"));
    assert_eq!(parsed.get("C").map(String::as_str), Some("\"quoted\""));
}

/// A leading underscore is a legal env-var name.
#[test]
fn parse_env_assignments_accepts_an_underscore_leading_key() {
    let parsed = parse_env_assignments("_PRIVATE=x\n").unwrap();

    assert_eq!(parsed.get("_PRIVATE").map(String::as_str), Some("x"));
}

// ----------------------------------------------------------------------
// Deferral through parameter expansions: `${SIB:-x}`, `${#SIB}`, `${SIB%y}`.
// ----------------------------------------------------------------------

/// The reference scan reports the variable the shell will read, not the raw
/// brace text. Before this, `${ZZZ:-fallback}` scanned as a variable named
/// `ZZZ:-fallback`, which is not a declared key, so no deferral happened and
/// the command ran before `ZZZ` resolved: the order-dependent result phase 3
/// fixed for `$ZZZ` survived for every parameter-expansion form.
#[test]
fn referenced_vars_reports_the_variable_a_parameter_expansion_reads() {
    assert_eq!(
        referenced_vars("${A:-x} ${#B} ${C%y} ${D/a/b} $E ${F}"),
        vec!["A", "B", "C", "D", "E", "F"]
    );
    // No leading identifier: reported whole, so the expander's error names
    // exactly what was written.
    assert_eq!(referenced_vars("${} ${1} ${#}"), vec!["", "1", "#"]);
}

/// `AAA` sorts before `ZZZ`, so without deferral the command runs first and
/// the shell default is what comes out. Measured on the first cut of 2.3.0:
/// `AAA=[got:fallback]`; renaming the keys so the sibling sorted first gave
/// `got:hello`. Same value, same ottofile, two answers.
#[test]
fn a_command_referencing_a_sibling_through_a_parameter_expansion_waits_for_it() {
    let mut envs = HashMap::new();
    envs.insert(
        "OTTO_TEST_AAA_DEFAULT".to_string(),
        "$(echo \"got:${OTTO_TEST_ZZZ_SIB:-fallback}\")".to_string(),
    );
    envs.insert(
        "OTTO_TEST_AAA_LEN".to_string(),
        "$(echo \"len:${#OTTO_TEST_ZZZ_SIB}\")".to_string(),
    );
    envs.insert(
        "OTTO_TEST_AAA_STRIP".to_string(),
        "$(echo \"strip:${OTTO_TEST_ZZZ_SIB%lo}\")".to_string(),
    );
    envs.insert("OTTO_TEST_ZZZ_SIB".to_string(), "hello".to_string());

    let result = evaluate_envs(&envs, None, &HashMap::new()).expect("the values must resolve");

    assert_eq!(
        result.get("OTTO_TEST_AAA_DEFAULT").map(String::as_str),
        Some("got:hello")
    );
    assert_eq!(result.get("OTTO_TEST_AAA_LEN").map(String::as_str), Some("len:5"));
    assert_eq!(result.get("OTTO_TEST_AAA_STRIP").map(String::as_str), Some("strip:hel"));
}

/// Outside a command the expander still resolves only `${NAME}`; a parameter
/// expansion there is the unresolvable reference it always was, and the error
/// names the whole reference (pinned end to end by `tests/dollar_escape_test.rs`).
#[test]
fn a_parameter_expansion_outside_a_command_still_fails_naming_the_whole_reference() {
    let mut context = HashMap::new();
    context.insert("OTTO_TEST_PRESENT".to_string(), "x".to_string());

    let error = resolve_env_variables("${OTTO_TEST_PRESENT:-fallback}", &context)
        .expect_err("otto templates do not implement parameter expansion")
        .to_string();

    assert!(
        error.contains("'OTTO_TEST_PRESENT:-fallback' not found"),
        "the error must name the whole reference, got: {error}"
    );
}
