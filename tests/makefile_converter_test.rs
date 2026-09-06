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
            "Makefile:2: warning: `BUILD_OWNER ?=` is conditional in make; otto always uses this value, ignoring any `BUILD_OWNER` already in the environment",
            "Makefile:3: warning: `BUILD_TAG ?=` is conditional in make; otto always uses this value, ignoring any `BUILD_TAG` already in the environment",
            "Makefile:28: warning: `type-check` is not `.PHONY` and has prerequisites; make skips its recipe when the target is newer than them and otto always runs it",
            "Makefile:31: warning: `lint` is not `.PHONY` and has prerequisites; make skips its recipe when the target is newer than them and otto always runs it",
            "Makefile:34: warning: `run-rest` is not `.PHONY` and has prerequisites; make skips its recipe when the target is newer than them and otto always runs it",
            "Makefile:38: warning: `clean` is not `.PHONY` and has prerequisites; make skips its recipe when the target is newer than them and otto always runs it",
        ],
    ),
    (
        "makefile-example",
        &[
            "Makefile:12: warning: `GIT_COMMIT ?=` is conditional in make; otto always uses this value, ignoring any `GIT_COMMIT` already in the environment",
            "Makefile:17: warning: `MAKEFILE_LIST` is a make-internal variable; it will be empty in otto",
            "Makefile:42: warning: `package` is not `.PHONY` and has prerequisites; make skips its recipe when the target is newer than them and otto always runs it",
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
    ("command-prefixes", &[]),
    ("phony-space", &[]),
    (
        "builtin-name-target",
        &[
            "Makefile:4: warning: target `Clean` is otto's built-in command name and cannot be a task; converted as `clean`",
        ],
    ),
    (
        "dollar-paren-target",
        &[
            "Makefile:9: warning: target `$(TARGETS)` is a make expansion; otto task names cannot be computed; the rule is skipped",
        ],
    ),
    (
        "expansion-dependency",
        &[
            "Makefile:15: warning: `link` is not `.PHONY` and has prerequisites; make skips its recipe when the target is newer than them and otto always runs it",
            "Makefile:15: warning: dependency `$(OBJS)` of `link` is a make expansion; otto task names cannot be computed",
            "Makefile:18: warning: `package` is not `.PHONY` and has prerequisites; make skips its recipe when the target is newer than them and otto always runs it",
            "Makefile:18: warning: dependency `headers` of `package` has no rule in this Makefile; otto will reject the edge",
            "Makefile:21: warning: `deps.d` is not `.PHONY` and has prerequisites; make skips its recipe when the target is newer than them and otto always runs it",
            "Makefile:21: warning: dependency `$(OBJS:%.o=%.d)` of `deps.d` is a make expansion; otto task names cannot be computed",
        ],
    ),
    (
        "substitution-ref-target",
        &[
            "Makefile:16: warning: target `$(SRCS:.c=.o)` is a make expansion; otto task names cannot be computed; the rule is skipped",
            "Makefile:19: warning: target `$(OBJS:%.o=%.d)` is a make expansion; otto task names cannot be computed; the rule is skipped",
            "Makefile:28: warning: unclosed `$(` or `${` in `$(BAD: headers`; the rule is skipped",
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
            "Makefile:24: warning: `build` is not `.PHONY` and has prerequisites; make skips its recipe when the target is newer than them and otto always runs it",
            "Makefile:24: warning: `UNSET_BY_MAKEFILE` is not defined in the Makefile; it will come from the environment",
            "Makefile:24: warning: `GIT_SHA` is not defined in the Makefile; it will come from the environment",
            "Makefile:30: warning: target-specific variable on `build` is not supported; the line is ignored",
            "Makefile:32: warning: `fmt check` declares 2 targets in one rule; each becomes its own task with the same recipe",
            "Makefile:35: warning: `install` is a double-colon rule; converted as an ordinary rule",
            "Makefile:35: warning: `install` is not `.PHONY` and has prerequisites; make skips its recipe when the target is newer than them and otto always runs it",
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

    let yaml = yaml_serde::to_string(&config).expect("ConfigSpec should serialize");
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
            yaml_serde::from_str(&expected_yaml).unwrap_or_else(|e| panic!("{expected_path} is not a ConfigSpec: {e}"));

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
        let reloaded: ConfigSpec = yaml_serde::from_str(&conversion.yaml).unwrap_or_else(|e| {
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

/// Every fixture's conversion must load through the *real* otto, not through
/// `yaml_serde`.
///
/// `test_every_fixture_round_trips_through_config_spec` deserializes the
/// converted YAML directly, which proves serde accepts the shape and nothing
/// more. Otto's binary load path does strictly more than deserialize: it gates
/// on `otto.api` and rejects reserved builtin param names, and this test
/// covers exactly that much by running the binary and asking it to enumerate
/// the tasks. It does NOT resolve the task graph: `--tasks --format json`
/// returns 0 on a file whose `before:` names a task that does not exist,
/// while running that task fails with "unknown dependency". That is by
/// construction - `makefiles/makefile-example` converts to exactly such a
/// file, its dangling `build` edge disclosed by a converter warning that
/// `test_every_converted_dependency_names_a_real_task` requires. Edge truth
/// is that test's job, not this one's.
#[test]
fn test_every_fixture_loads_through_the_real_otto() {
    for name in converting_fixtures() {
        let conversion = convert_fixture(&name);

        let work = TempDir::new().expect("scratch dir");
        let ottofile = work.path().join("otto.yml");
        fs::write(&ottofile, &conversion.yaml).expect("write converted ottofile");

        let home = TempDir::new().expect("scratch OTTO_HOME");
        let output = common::otto_std_cmd(home.path())
            .arg("--ottofile")
            .arg(&ottofile)
            .arg("--tasks")
            .arg("--format")
            .arg("json")
            .current_dir(work.path())
            .output()
            .expect("otto --tasks should run");

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            output.status.success(),
            "otto refuses to load the conversion of makefiles/{name}/Makefile \
             (rc={:?})\nstderr:\n{stderr}\nyaml:\n{}",
            output.status.code(),
            conversion.yaml
        );

        // Loading is not enough on its own: otto must also see the tasks the
        // converter claims it emitted. An empty task list would otherwise pass.
        assert!(
            !conversion.config.tasks.is_empty(),
            "makefiles/{name}/Makefile converted to zero tasks"
        );

        // Exact key membership, not `stdout.contains(task_name)`.
        //
        // The substring form was hollow and a review panel proved it: `--tasks
        // --format json` keys the object by task name, so renaming every key to
        // `<name>-shadow` leaves the original name a substring of the new one and
        // the assertion green. Parsing and comparing the key set is the only form
        // that distinguishes "otto lists this task" from "these characters appear
        // somewhere in the output".
        let listed: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("otto --tasks --format json is not JSON for makefiles/{name}: {e}\n{stdout}"));
        let listed = listed
            .as_object()
            .unwrap_or_else(|| panic!("otto --tasks --format json is not an object for makefiles/{name}:\n{stdout}"));
        for task_name in conversion.config.tasks.keys() {
            assert!(
                listed.contains_key(task_name.as_str()),
                "otto loaded the conversion of makefiles/{name}/Makefile but does not list \
                 task '{task_name}'. listed: {:?}",
                listed.keys().collect::<Vec<_>>()
            );
        }
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

/// `NON_CONVERTING_FIXTURES` is the one remaining way to remove a fixture from
/// every loop in this file, and it is one line long.
///
/// That is the same silent opt-out this suite has now closed twice: first the
/// `continue` on a missing golden, then the hand-maintained fixture list. Left
/// unguarded it is the third turn of the same screw - a new directory plus one
/// entry here gives `0 failed` with zero coverage. Pinning the list by value
/// makes adding to it a deliberate, reviewable diff rather than a line nobody
/// notices.
#[test]
fn the_non_converting_opt_out_holds_exactly_one_known_name() {
    assert_eq!(
        NON_CONVERTING_FIXTURES,
        ["space-indented-recipe"],
        "a fixture may only be excluded from the directory-driven loops with a deliberate \
         change here AND a dedicated test proving what it does instead - \
         `space-indented-recipe` has two (`test_space_indented_recipe_is_rejected_by_name` \
         and `test_space_indented_recipe_fails_the_command_with_the_line_number`)"
    );
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
    let reloaded: ConfigSpec = yaml_serde::from_str(&conversion.yaml).expect("Failed to deserialize from YAML");

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

/// `.PHONY: $(TARGETS)` must not make `--strict` reject a correct Makefile.
///
/// The file-rule warning fires on `!target.is_phony`, and `.PHONY` is not
/// variable-expanded by this parser, so every target declared through a variable
/// looked non-phony. A review panel found it: three correct lines, two wrong
/// warnings, exit 1 under `--strict`.
///
/// Both directions are driven in one test, because the fix is only correct if it
/// suppresses the false positive *without* suppressing the true one - and
/// "stop warning" is the trivially wrong way to make the first half pass.
#[test]
fn an_unresolvable_phony_suppresses_the_file_rule_warning_but_a_literal_one_does_not() {
    let variable_phony = "TARGETS = build test\n\n.PHONY: $(TARGETS)\n\nbuild: dep\n\techo building\n\ntest: dep\n\techo testing\n\ndep:\n\techo dep\n";
    let (code, _stdout, stderr) = run_convert(variable_phony, &["--strict"]);
    assert_eq!(
        code, 0,
        "`.PHONY: $(TARGETS)` is a correct Makefile and must convert under --strict:\n{stderr}"
    );
    assert!(
        !stderr.contains("is not `.PHONY` and has prerequisites"),
        "phony-ness is unknown here, so the heuristic must not assert it:\n{stderr}"
    );

    // The control: a literal `.PHONY` leaves the set complete, so a genuine file
    // rule must still be reported.
    let literal_phony = ".PHONY: clean\n\napp.tar.gz: src.txt\n\ttar czf app.tar.gz src.txt\n\nsrc.txt:\n\techo hi > src.txt\n\nclean:\n\trm -f app.tar.gz\n";
    let (_code, _stdout, stderr) = run_convert(literal_phony, &[]);
    assert!(
        stderr.contains("`app.tar.gz` is not `.PHONY` and has prerequisites"),
        "a real file rule under a literal .PHONY must still warn:\n{stderr}"
    );
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

/// `is_phony` was set from `.PHONY` and read by nobody, so a make file rule
/// converted into a task that always runs with nothing said about it. make
/// skips a non-phony target's recipe when the target is newer than its
/// prerequisites; otto has no equivalent check.
///
/// The signal is PREREQUISITES, not the target's name. A first attempt guessed
/// from the name (a `/` or a dot-extension) and was wrong in both directions:
/// `myapp: main.o`, the canonical Unix file rule, got no warning at all, while
/// a bare `deploy.prod` got a false one. These four cases are exactly that pair
/// plus the two negatives.
#[test]
fn test_a_non_phony_target_with_prerequisites_warns() {
    let conversion = convert("myapp: main.o\n\tcc -o myapp main.o\n".to_string());

    assert!(
        conversion
            .warnings
            .iter()
            .any(|w| w.contains("`myapp` is not `.PHONY` and has prerequisites")),
        "the canonical Unix file rule must warn: {:?}",
        conversion.warnings
    );
}

#[test]
fn test_a_dotted_name_without_prerequisites_does_not_warn() {
    let conversion = convert("deploy.prod:\n\techo deploying\n".to_string());

    assert!(
        !conversion.warnings.iter().any(|w| w.contains("has prerequisites")),
        "a dotted name is namespacing, not a file rule; guessing from the name is what \
         this replaced: {:?}",
        conversion.warnings
    );
}

#[test]
fn test_declaring_it_phony_silences_the_warning() {
    let conversion = convert(".PHONY: myapp\nmyapp: main.o\n\tcc -o myapp main.o\n".to_string());

    assert!(
        !conversion.warnings.iter().any(|w| w.contains("has prerequisites")),
        "`.PHONY` is exactly the author saying this is not a file: {:?}",
        conversion.warnings
    );
}

#[test]
fn test_prerequisites_without_a_recipe_do_not_warn() {
    let conversion = convert("all: myapp\n\nmyapp:\n\techo hi\n".to_string());

    assert!(
        !conversion.warnings.iter().any(|w| w.contains("has prerequisites")),
        "a target with prerequisites and no recipe is a dependency declaration; there is \
         nothing for otto to run differently: {:?}",
        conversion.warnings
    );
}

/// A conversion that produced no tasks did nothing, and said so only by
/// printing `tasks: {}` at exit 0 - including under `--strict`, whose whole job
/// is to refuse a lossy conversion.
#[test]
fn test_a_conversion_with_no_tasks_says_so() {
    let conversion = convert(String::new());

    assert!(
        conversion
            .warnings
            .iter()
            .any(|w| w.contains("no targets were converted")),
        "an empty conversion must be audible: {:?}",
        conversion.warnings
    );
}

#[test]
fn test_strict_fails_a_conversion_that_produced_no_tasks() {
    let (code, stdout, stderr) = run_convert("", &["--strict"]);

    assert_eq!(code, 1, "--strict must refuse an empty conversion. stderr: {stderr}");
    assert!(stdout.is_empty(), "--strict must not emit a conversion: {stdout}");
    assert!(stderr.contains("no targets were converted"), "{stderr}");
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

/// Every converting fixture's output loads through otto itself. The converter's
/// consumer is the loader, and nothing checked that the two agreed: phase 5
/// reserved the built-in command names at load, `Convert` kept emitting a
/// `Clean:` target under that name, and the output it wrote at exit 0 failed
/// `otto --tasks` with "defines reserved builtin command name". A golden
/// compared as a `ConfigSpec` cannot see that; running the binary can.
#[test]
fn test_every_fixture_output_loads_through_otto() {
    for name in converting_fixtures() {
        let conversion = convert_fixture(&name);
        let temp = TempDir::new().expect("tempdir");
        let ottofile = temp.path().join("otto.yml");
        fs::write(&ottofile, &conversion.yaml).expect("write converted ottofile");

        let output = common::otto_cmd(temp.path())
            .arg("-o")
            .arg(&ottofile)
            .arg("--tasks")
            .output()
            .expect("otto --tasks");
        assert!(
            output.status.success(),
            "makefiles/{name}/Makefile converted to an ottofile otto itself rejects:\n{}\n--- output ---\n{}",
            String::from_utf8_lossy(&output.stderr),
            conversion.yaml
        );
    }
}
