//! What `otto Convert` is allowed to do to a Makefile.
//!
//! Every fixture lives in `makefiles/<name>/Makefile`. A fixture that converts
//! carries an `expected.yml` beside it, compared as a `ConfigSpec` rather than
//! as text: `otto.envs` is a `HashMap`, so its serialized key order varies
//! between runs and a string diff would be flaky for no gain.
//!
//! The old suite asserted only that the output was non-empty, which is why
//! `$(shell ...)` and `$(VAR)` survived conversion for as long as they did.

mod common;

use otto::ConfigSpec;
use otto::makefile::{MakefileParser, OttoConverter};
use std::fs;
use std::io::Write;
use std::process::Stdio;
use tempfile::TempDir;

/// The warnings each converting fixture must produce. An empty list is the
/// strongest claim in the file: this Makefile converts with nothing lost.
///
/// This table is a lookup keyed off `converting_fixtures()`, NOT the list of
/// fixtures. It used to be the list, and a hand-maintained transcription of a
/// directory silently skips whatever it forgets: a ninth fixture directory
/// with no golden and no entry here passed the whole suite.
const CONVERTING_FIXTURES: &[(&str, &[&str])] = &[
    ("go-build-project", &[]),
    (
        "python-poetry-service",
        &[
            "Makefile:2: warning: `CERTS_DIR ?=` is conditional in make; otto always uses this value, ignoring any `CERTS_DIR` already in the environment",
            "Makefile:42: warning: `CLIENT_ID` is not defined in the Makefile; it will come from the environment",
            "Makefile:42: warning: `CLIENT_ID` is not defined in the Makefile; it will come from the environment",
        ],
    ),
    ("python-pre-commit", &[]),
    (
        "docker-compose-service",
        &[
            "Makefile:2: warning: `PYPI_USERNAME ?=` is conditional in make; otto always uses this value, ignoring any `PYPI_USERNAME` already in the environment",
            "Makefile:3: warning: `PYPI_PASSWORD ?=` is conditional in make; otto always uses this value, ignoring any `PYPI_PASSWORD` already in the environment",
        ],
    ),
    (
        "makefile-example",
        &[
            "Makefile:12: warning: `GIT_COMMIT ?=` is conditional in make; otto always uses this value, ignoring any `GIT_COMMIT` already in the environment",
            "Makefile:17: warning: `MAKEFILE_LIST` is a make-internal variable; it will be empty in otto",
            "Makefile:42: warning: `PROJECT_NAME` is not defined in the Makefile; it will come from the environment",
            "Makefile:42: warning: dependency `build` of `package` has no rule in this Makefile; otto will reject the edge",
        ],
    ),
    (
        "pattern-rules",
        &[
            "Makefile:7: warning: pattern rule `%.o` is not supported; the rule is skipped",
            "Makefile:10: warning: static pattern rule `objects` is not supported; the rule is skipped",
            "Makefile:13: warning: special target `.SUFFIXES` is not converted",
        ],
    ),
    (
        "unsupported-constructs",
        &[
            "Makefile:9: warning: `FLAGS +=` cannot append in otto; only the appended text is kept",
            "Makefile:10: warning: `export` on `REGISTRY` dropped; otto exports every declared env",
            "Makefile:11: warning: variable `OBJECTS` uses a make function otto cannot evaluate; the variable is dropped",
            "Makefile:13: warning: `include` is not supported; the line is ignored",
            "Makefile:15: warning: conditional block skipped; its body is not converted",
            "Makefile:19: warning: multi-line variable (define) is not supported; skipped",
            "Makefile:24: warning: `UNSET_BY_MAKEFILE` is not defined in the Makefile; it will come from the environment",
            "Makefile:24: warning: `GIT_SHA` is not defined in the Makefile; it will come from the environment",
            "Makefile:30: warning: target-specific variable on `build` is not supported; the line is ignored",
            "Makefile:32: warning: `fmt check` declares 2 targets in one rule; each becomes its own task with the same recipe",
            "Makefile:35: warning: `install` is a double-colon rule; converted as an ordinary rule",
            "Makefile:38: warning: duplicate target `fmt` overrides the rule at line 32, discarding its dependencies and recipe",
        ],
    ),
];

/// Fixtures that must NOT convert, and so are excluded from every
/// directory-driven loop below. `space-indented-recipe` is rejected by the
/// parser on purpose; `test_space_indented_recipe_is_rejected_by_name` and
/// `test_space_indented_recipe_fails_the_command_with_the_line_number` cover it.
const NON_CONVERTING_FIXTURES: &[&str] = &["space-indented-recipe"];

/// Every fixture that must convert, read from `makefiles/` rather than
/// transcribed. The criterion this file implements says "every fixture in
/// `makefiles/`", so the directory is the source of truth: a new fixture is
/// enrolled in every test below by existing, and cannot opt itself out by
/// being forgotten.
fn converting_fixtures() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir("makefiles")
        .expect("makefiles/ must exist")
        .map(|entry| entry.expect("makefiles/ entry must be readable"))
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !NON_CONVERTING_FIXTURES.contains(&name.as_str()))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "makefiles/ has no fixture directories");
    names
}

/// The warnings `name` must produce, or a panic naming the fixture. A fixture
/// with no entry is the loudest failure in the file, because it is what adding
/// a directory forgets.
fn expected_warnings(name: &str) -> &'static [&'static str] {
    CONVERTING_FIXTURES
        .iter()
        .find(|(fixture, _)| *fixture == name)
        .map(|(_, warnings)| *warnings)
        .unwrap_or_else(|| {
            panic!(
                "makefiles/{name}/ has no entry in CONVERTING_FIXTURES; add one \
                 (an empty warning list means it converts cleanly)"
            )
        })
}

struct Conversion {
    config: ConfigSpec,
    yaml: String,
    warnings: Vec<String>,
}

fn convert(content: String) -> Conversion {
    let mut parser = MakefileParser::new(content);
    let ast = parser.parse().expect("Makefile should convert");

    let mut converter = OttoConverter::new(ast);
    let config = converter.convert().expect("conversion should succeed");

    let mut diagnostics: Vec<_> = parser
        .diagnostics()
        .iter()
        .chain(converter.diagnostics())
        .cloned()
        .collect();
    diagnostics.sort_by_key(|d| d.line);

    let yaml = serde_yaml::to_string(&config).expect("ConfigSpec should serialize");
    Conversion {
        config,
        yaml,
        warnings: diagnostics.iter().map(|d| d.to_string()).collect(),
    }
}

fn convert_fixture(name: &str) -> Conversion {
    let path = format!("makefiles/{name}/Makefile");
    let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read {path}: {e}"));
    convert(content)
}

#[test]
fn test_every_fixture_produces_exactly_its_expected_warnings() {
    for name in converting_fixtures() {
        let conversion = convert_fixture(&name);
        assert_eq!(
            conversion.warnings,
            expected_warnings(&name),
            "warnings changed for makefiles/{name}/Makefile"
        );
    }
}

#[test]
fn test_no_fixture_leaks_make_syntax_into_its_output() {
    for name in converting_fixtures() {
        let conversion = convert_fixture(&name);

        assert!(
            !conversion.yaml.contains("$(shell"),
            "makefiles/{name}/Makefile left a $(shell in the output:\n{}",
            conversion.yaml
        );

        // `$(VAR)` in a bash action is command substitution: it runs a program
        // called VAR, prints "command not found", expands to nothing, and the
        // task still succeeds.
        let leaks = make_variable_references(&conversion.yaml);
        assert!(
            leaks.is_empty(),
            "makefiles/{name}/Makefile left make references {leaks:?} in the output"
        );
    }
}

/// Every `$(IDENT)` left in converted output. A make function call such as
/// `$(wildcard *.c)` is deliberately left alone (with a warning), so only bare
/// identifiers count as a leak.
fn make_variable_references(yaml: &str) -> Vec<String> {
    let mut found = Vec::new();
    let chars: Vec<char> = yaml.chars().collect();

    for i in 0..chars.len().saturating_sub(2) {
        if chars[i] != '$' || chars[i + 1] != '(' {
            continue;
        }
        let Some(close) = chars[i + 2..].iter().position(|c| *c == ')') else {
            continue;
        };
        let inner: String = chars[i + 2..i + 2 + close].iter().collect();
        let identifier = !inner.is_empty()
            && inner.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if identifier {
            found.push(format!("$({inner})"));
        }
    }

    found
}

/// Every fixture, against its committed golden - no exceptions, and no silent
/// skip at either level. This used to `continue` when `expected.yml` was
/// absent, which left 5 of the 7 fixtures compared against nothing. Making
/// that a panic moved the skip one level up rather than removing it: the loop
/// still walked a hand-maintained list, so a fixture directory missing from
/// the list was never reached to panic about. The criterion says "every
/// fixture in `makefiles/`", and now so does the loop.
#[test]
fn test_every_fixture_matches_its_expected_output() {
    for name in converting_fixtures() {
        let expected_path = format!("makefiles/{name}/expected.yml");
        let expected_yaml = fs::read_to_string(&expected_path).unwrap_or_else(|e| {
            panic!(
                "{expected_path} is missing ({e}); every fixture in makefiles/ must \
                 carry a golden, or this test compares it against nothing"
            )
        });

        let expected: ConfigSpec =
            serde_yaml::from_str(&expected_yaml).unwrap_or_else(|e| panic!("{expected_path} is not a ConfigSpec: {e}"));

        assert_eq!(
            convert_fixture(&name).config,
            expected,
            "conversion of makefiles/{name}/Makefile no longer matches {expected_path}"
        );
    }
}

#[test]
fn test_every_fixture_round_trips_through_config_spec() {
    for name in converting_fixtures() {
        let conversion = convert_fixture(&name);

        // Loading is the real test: `ConfigSpec` denies unknown fields, so this
        // proves the converter emits keys otto actually accepts.
        let reloaded: ConfigSpec = serde_yaml::from_str(&conversion.yaml).unwrap_or_else(|e| {
            panic!(
                "converted makefiles/{name}/Makefile does not load: {e}\n{}",
                conversion.yaml
            )
        });

        assert_eq!(
            reloaded, conversion.config,
            "makefiles/{name}/Makefile does not survive a serialize/load round trip"
        );
    }
}

#[test]
fn test_every_converted_dependency_names_a_real_task() {
    for name in converting_fixtures() {
        let conversion = convert_fixture(&name);

        for (task_name, task) in &conversion.config.tasks {
            for edge in &task.before {
                if conversion.config.tasks.contains_key(&edge.task) {
                    continue;
                }
                // An edge otto will reject at load time is allowed only when the
                // conversion said so out loud.
                assert!(
                    conversion
                        .warnings
                        .iter()
                        .any(|w| w.contains(&format!("dependency `{}` of `{task_name}`", edge.task))),
                    "makefiles/{name}/Makefile: task `{task_name}` depends on `{}`, which is not a task, with no warning: {:?}",
                    edge.task,
                    conversion.warnings
                );
            }
        }
    }
}

/// The other direction: a name in `CONVERTING_FIXTURES` with no directory
/// behind it. `expected_warnings` catches the missing entry; nothing else
/// catches the entry whose fixture was renamed or deleted, and a stale row
/// would sit there asserting nothing.
#[test]
fn test_no_expected_warnings_entry_outlives_its_fixture() {
    let present = converting_fixtures();
    for (name, _) in CONVERTING_FIXTURES {
        assert!(
            present.iter().any(|fixture| fixture == name),
            "CONVERTING_FIXTURES names `{name}`, but makefiles/{name}/ does not exist \
             (or is listed in NON_CONVERTING_FIXTURES); the entry asserts nothing"
        );
    }
}

#[test]
fn test_space_indented_recipe_is_rejected_by_name() {
    let content = fs::read_to_string("makefiles/space-indented-recipe/Makefile").unwrap();

    let mut parser = MakefileParser::new(content);
    let err = parser.parse().expect_err("a space-indented recipe must not convert");

    let message = err.to_string();
    assert!(message.contains("Makefile:6"), "{message}");
    assert!(message.contains("indented with spaces"), "{message}");
    assert!(
        message.contains("will not guess whether this is a recipe or a target"),
        "{message}"
    );
}

#[test]
fn test_pattern_rule_produces_a_warning_and_no_task() {
    let conversion = convert_fixture("pattern-rules");

    assert!(
        conversion.config.tasks.keys().all(|name| !name.contains('%')),
        "a pattern rule became a task: {:?}",
        conversion.config.tasks.keys().collect::<Vec<_>>()
    );
    assert!(
        conversion.warnings.iter().any(|w| w.contains("pattern rule `%.o`")),
        "{:?}",
        conversion.warnings
    );
}

#[test]
fn test_roundtrip_conversion() {
    // Test that we can convert a Makefile to Otto and then serialize/deserialize
    let simple_makefile = r#"
VAR := value

.DEFAULT_GOAL := build

.PHONY: build clean

# Build the project
build:
	echo "Building..."
	mkdir -p dist

clean:
	rm -rf dist
"#;

    let conversion = convert(simple_makefile.to_string());
    let reloaded: ConfigSpec = serde_yaml::from_str(&conversion.yaml).expect("Failed to deserialize from YAML");

    assert!(conversion.warnings.is_empty(), "{:?}", conversion.warnings);
    assert_eq!(reloaded, conversion.config);
    assert_eq!(reloaded.otto.tasks, vec!["build".to_string()]);
    assert_eq!(reloaded.tasks.len(), 2);
}

// --- the CLI surface ------------------------------------------------------

fn run_convert(makefile: &str, args: &[&str]) -> (i32, String, String) {
    // `Convert` doesn't touch the store, but it's isolated anyway so
    // isolation can't drift file by file. `assert_cmd::Command` doesn't
    // expose raw stdio piping, so this goes through `common::otto_std_cmd`,
    // the `std::process::Command` twin of `common::otto_cmd`.
    let home = TempDir::new().expect("failed to create scratch OTTO_HOME");
    let mut child = common::otto_std_cmd(home.path())
        .arg("Convert")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("otto Convert should start");

    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(makefile.as_bytes())
        .expect("write stdin");

    let output = child.wait_with_output().expect("otto Convert should finish");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn test_warnings_go_to_stderr_and_do_not_fail_the_conversion() {
    let makefile = fs::read_to_string("makefiles/pattern-rules/Makefile").unwrap();

    let (code, stdout, stderr) = run_convert(&makefile, &[]);

    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("tasks:"), "{stdout}");
    assert!(stderr.contains("warning: pattern rule"), "{stderr}");
}

#[test]
fn test_strict_turns_warnings_into_a_nonzero_exit() {
    let makefile = fs::read_to_string("makefiles/pattern-rules/Makefile").unwrap();

    let (code, stdout, stderr) = run_convert(&makefile, &["--strict"]);

    assert_eq!(code, 1, "--strict must fail on warnings. stderr: {stderr}");
    assert!(stdout.is_empty(), "--strict must not emit a conversion: {stdout}");
    assert!(stderr.contains("--strict was given"), "{stderr}");
}

/// The task's own inline `##` doc string beats a preceding `#` banner.
///
/// It used to be the other way round, so a section banner two lines above a
/// rule was adopted as that rule's help text - and the golden generated from
/// that output committed the bug: `help: HELP` (its own `##` says "Help me."),
/// `build-linux: Cross compilation` ("Build linux artifact"), `dep: helpers`
/// ("Install deps"). The comment at the fix site already named `##` as the
/// self-documenting-makefile convention the parser honors; the code just did
/// not follow it.
#[test]
fn test_a_rules_own_doc_string_beats_a_preceding_banner() {
    let conversion = convert("# Banner\nbuild: ## Build the thing\n\techo hi\n".to_string());

    assert_eq!(
        conversion.config.tasks["build"].help.as_deref(),
        Some("Build the thing"),
        "the rule's own `##` names the target; the banner names the section"
    );
}

/// A banner is still used when the rule has no `##` of its own.
#[test]
fn test_a_banner_is_still_the_fallback_when_a_rule_has_no_doc_string() {
    let conversion = convert("# Banner\nbuild:\n\techo hi\n".to_string());

    assert_eq!(conversion.config.tasks["build"].help.as_deref(), Some("Banner"));
}

/// `is_phony` was set from `.PHONY` and read by nobody, so a make *file rule*
/// converted into a task that always runs with nothing said about it. make
/// skips a file target's recipe when the file is newer than its prerequisites;
/// otto has no equivalent check.
///
/// The signal is the path shape, not "everything outside `.PHONY`": most
/// authors never list every ordinary target, so warning on all of them would be
/// noise. `dist/app` and `app.tar.gz` are file rules; `build` and `check-fmt`
/// are not.
#[test]
fn test_a_file_shaped_target_outside_phony_warns() {
    let conversion = convert("app.tar.gz: build\n\ttar -cz .\n\nbuild:\n\techo hi\n".to_string());

    assert!(
        conversion
            .warnings
            .iter()
            .any(|w| w.contains("`app.tar.gz` is a file target, not `.PHONY`")),
        "a file-shaped target outside .PHONY must say make and otto disagree: {:?}",
        conversion.warnings
    );
    assert!(
        !conversion
            .warnings
            .iter()
            .any(|w| w.contains("`build` is a file target")),
        "a plain-word target is not a file rule: {:?}",
        conversion.warnings
    );
}

/// And declaring it phony silences it, which is what makes the field read
/// rather than merely present.
#[test]
fn test_a_file_shaped_target_declared_phony_does_not_warn() {
    let conversion = convert(".PHONY: app.tar.gz\napp.tar.gz:\n\ttar -cz .\n".to_string());

    assert!(
        !conversion.warnings.iter().any(|w| w.contains("is a file target")),
        "`.PHONY` is exactly the author saying this is not a file: {:?}",
        conversion.warnings
    );
}

/// `?=` converted as a plain `=`, with no warning at all.
///
/// otto's `envs:` is "the system environment minus every declared key"
/// (`cfg/env.rs`), so a declared key unconditionally shadows the ambient one -
/// the exact opposite of what `?=` means. Measured against real make on
/// `VERSION ?= 1.0`:
///
///   VERSION=9.9 make -s show  -> 9.9
///   VERSION=9.9 otto show     -> 1.0
///
/// otto does not gain make's conditional-assignment semantics here: the
/// contract for `envs:` is not this test's to change. What it gains is saying
/// so. `+=` already warned for this same class of loss and `?=` warned for
/// nothing, contradicting Phase 7's own "every silent corruption above becomes
/// a warning at minimum".
#[test]
fn test_conditional_assignment_warns_that_the_environment_no_longer_wins() {
    let conversion = convert("VERSION ?= 1.0\n\nshow:\n\t@echo $(VERSION)\n".to_string());

    assert_eq!(
        conversion.warnings,
        vec![
            "Makefile:1: warning: `VERSION ?=` is conditional in make; otto always uses this \
             value, ignoring any `VERSION` already in the environment"
        ],
        "a `?=` that silently becomes `=` must say so"
    );
}

/// And `--strict` promotes it, per the same criterion's second half.
#[test]
fn test_strict_fails_on_a_conditional_assignment() {
    let (code, stdout, stderr) = run_convert("VERSION ?= 1.0\n\nshow:\n\t@echo $(VERSION)\n", &["--strict"]);

    assert_eq!(code, 1, "--strict must fail on a `?=` warning. stderr: {stderr}");
    assert!(stdout.is_empty(), "--strict must not emit a conversion: {stdout}");
    assert!(
        stderr.contains("VERSION ?="),
        "the warning must name the variable: {stderr}"
    );
}

#[test]
fn test_strict_passes_a_makefile_that_converts_cleanly() {
    let makefile = fs::read_to_string("makefiles/python-pre-commit/Makefile").unwrap();

    let (code, stdout, stderr) = run_convert(&makefile, &["--strict"]);

    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("tasks:"), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn test_space_indented_recipe_fails_the_command_with_the_line_number() {
    let makefile = fs::read_to_string("makefiles/space-indented-recipe/Makefile").unwrap();

    let (code, stdout, stderr) = run_convert(&makefile, &[]);

    assert_eq!(code, 1, "stderr: {stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("Makefile:6"), "{stderr}");
    assert!(stderr.contains("indented with spaces"), "{stderr}");
}
