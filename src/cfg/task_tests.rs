#![cfg(test)]

use super::*;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_foreach_resolve_items_list() {
    let foreach = ForeachSpec {
        items: vec!["dev".to_string(), "staging".to_string(), "prod".to_string()],
        ..Default::default()
    };

    let cwd = PathBuf::from("/tmp");
    let items = foreach.resolve_items(&cwd).unwrap();

    assert_eq!(items.len(), 3);
    assert_eq!(items[0].identifier, "dev");
    assert_eq!(items[0].value, "dev");
    assert_eq!(items[1].identifier, "staging");
    assert_eq!(items[2].identifier, "prod");
}

#[test]
fn test_foreach_resolve_items_range() {
    let foreach = ForeachSpec {
        range: Some("1-5".to_string()),
        ..Default::default()
    };

    let cwd = PathBuf::from("/tmp");
    let items = foreach.resolve_items(&cwd).unwrap();

    assert_eq!(items.len(), 5);
    assert_eq!(items[0].identifier, "1");
    assert_eq!(items[0].value, "1");
    assert_eq!(items[4].identifier, "5");
    assert_eq!(items[4].value, "5");
}

#[test]
fn test_foreach_resolve_items_range_zero_padded() {
    let foreach = ForeachSpec {
        range: Some("1-12".to_string()),
        ..Default::default()
    };

    let cwd = PathBuf::from("/tmp");
    let items = foreach.resolve_items(&cwd).unwrap();

    assert_eq!(items.len(), 12);
    assert_eq!(items[0].identifier, "01"); // Zero-padded to match width of "12"
    assert_eq!(items[0].value, "1");
    assert_eq!(items[9].identifier, "10");
    assert_eq!(items[11].identifier, "12");
}

#[test]
fn test_foreach_resolve_items_glob() {
    let temp_dir = TempDir::new().unwrap();
    let dir = temp_dir.path();

    // Create test files
    std::fs::write(dir.join("a.txt"), "").unwrap();
    std::fs::write(dir.join("b.txt"), "").unwrap();
    std::fs::write(dir.join("c.txt"), "").unwrap();
    std::fs::write(dir.join("skip.md"), "").unwrap(); // Should not match

    let foreach = ForeachSpec {
        glob: Some("*.txt".to_string()),
        ..Default::default()
    };

    let items = foreach.resolve_items(dir).unwrap();

    assert_eq!(items.len(), 3);
    // Should be sorted alphabetically
    assert_eq!(items[0].identifier, "a.txt");
    assert_eq!(items[1].identifier, "b.txt");
    assert_eq!(items[2].identifier, "c.txt");
}

#[test]
fn test_foreach_max_items_limit() {
    let foreach = ForeachSpec {
        range: Some("1-100".to_string()),
        max_items: 10,
        ..Default::default()
    };

    let cwd = PathBuf::from("/tmp");
    let result = foreach.resolve_items(&cwd);

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("exceeding max_items"));
}

#[test]
fn test_foreach_empty_items_filtered() {
    let foreach = ForeachSpec {
        items: vec!["a".to_string(), "".to_string(), "  ".to_string(), "b".to_string()],
        ..Default::default()
    };

    let cwd = PathBuf::from("/tmp");
    let items = foreach.resolve_items(&cwd).unwrap();

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].identifier, "a");
    assert_eq!(items[1].identifier, "b");
}

#[test]
fn test_foreach_invalid_range_format() {
    let foreach = ForeachSpec {
        range: Some("invalid".to_string()),
        ..Default::default()
    };

    let cwd = PathBuf::from("/tmp");
    let result = foreach.resolve_items(&cwd);

    assert!(result.is_err());
}

#[test]
fn test_foreach_range_start_greater_than_end() {
    let foreach = ForeachSpec {
        range: Some("10-5".to_string()),
        ..Default::default()
    };

    let cwd = PathBuf::from("/tmp");
    let result = foreach.resolve_items(&cwd);

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("start (10) > end (5)"));
}

#[test]
fn test_foreach_requires_source() {
    let foreach = ForeachSpec::default();

    let cwd = PathBuf::from("/tmp");
    let result = foreach.resolve_items(&cwd);

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("foreach requires glob, items, or range"));
}

// ------------------------------------------------------------------
// Command source (design doc 2026-08-28, Phase 6)
// ------------------------------------------------------------------

fn command_foreach(command: &str) -> ForeachSpec {
    ForeachSpec {
        command: Some(command.to_string()),
        var_name: "svc".to_string(),
        ..Default::default()
    }
}

#[test]
fn test_foreach_command_items_are_trimmed_non_empty_lines() {
    let foreach = command_foreach("printf 'alpha\n\n  beta  \n'");
    let items = foreach
        .resolve_command_items("up", &PathBuf::from("."), &HashMap::new())
        .unwrap();

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].identifier, "alpha");
    assert_eq!(items[1].identifier, "beta");
    assert_eq!(items[1].value, "beta");
}

#[test]
fn test_foreach_command_sanitizes_identifiers_like_glob_does() {
    let foreach = command_foreach("printf 'two words\n'");
    let items = foreach
        .resolve_command_items("up", &PathBuf::from("."), &HashMap::new())
        .unwrap();

    assert_eq!(items[0].identifier, "two_words");
    assert_eq!(items[0].value, "two words", "the value keeps the original spacing");
}

#[test]
fn test_foreach_command_nonzero_exit_names_task_and_command() {
    let foreach = command_foreach("exit 7");
    let err = foreach
        .resolve_command_items("up", &PathBuf::from("."), &HashMap::new())
        .unwrap_err()
        .to_string();

    assert!(err.contains("up"), "{err}");
    assert!(err.contains("exit 7"), "{err}");
    assert!(err.contains("exit code 7"), "{err}");
}

#[test]
fn test_foreach_command_zero_lines_is_an_empty_expansion_not_an_error() {
    let foreach = command_foreach("true");
    let items = foreach
        .resolve_command_items("up", &PathBuf::from("."), &HashMap::new())
        .unwrap();

    assert!(items.is_empty());
}

#[test]
fn test_foreach_command_respects_max_items() {
    let foreach = ForeachSpec {
        max_items: 2,
        ..command_foreach("printf 'a\nb\nc\n'")
    };
    let err = foreach
        .resolve_command_items("up", &PathBuf::from("."), &HashMap::new())
        .unwrap_err()
        .to_string();

    assert!(err.contains("max_items"), "{err}");
}

#[test]
fn test_foreach_command_is_exclusive_with_static_sources() {
    let foreach = ForeachSpec {
        items: vec!["x".to_string()],
        range: Some("1-2".to_string()),
        ..command_foreach("printf 'a\n'")
    };

    let err = foreach.validate_sources("up").unwrap_err().to_string();
    assert!(err.contains("Task 'up'"), "{err}");
    assert!(err.contains("items"), "{err}");
    assert!(err.contains("range"), "{err}");

    // and the same error blocks resolution, not just load-time validation
    assert!(
        foreach
            .resolve_command_items("up", &PathBuf::from("."), &HashMap::new())
            .is_err()
    );
}

#[test]
fn test_foreach_static_sources_still_validate_clean() {
    let foreach = ForeachSpec {
        items: vec!["x".to_string()],
        glob: Some("*.sh".to_string()),
        ..Default::default()
    };
    // Only a `command:` source is exclusive; the pre-existing glob/items
    // precedence is untouched by this phase.
    assert!(foreach.validate_sources("up").is_ok());
}

#[test]
fn test_resolve_items_refuses_a_command_source() {
    let foreach = command_foreach("printf 'a\n'");
    let err = foreach.resolve_items(&PathBuf::from(".")).unwrap_err().to_string();

    assert!(
        err.contains("must be resolved through otto's foreach resolver"),
        "{err}"
    );
}

#[test]
fn test_expand_foreach_with_items_names_subtasks_and_injects_vars() {
    let mut task = TaskSpec::new(
        "up".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        ParamSpecs::new(),
        "echo ${svc}".to_string(),
    );
    task.foreach = Some(command_foreach("unused: items are supplied"));

    let items = vec![
        ForeachItem {
            identifier: "alpha".to_string(),
            value: "alpha".to_string(),
        },
        ForeachItem {
            identifier: "beta".to_string(),
            value: "beta".to_string(),
        },
    ];
    let subtasks = task.expand_foreach_with_items(&items).unwrap();

    assert_eq!(subtasks.len(), 2);
    assert_eq!(subtasks[0].name, "up:alpha");
    assert_eq!(subtasks[1].name, "up:beta");
    assert_eq!(subtasks[1].envs.get("svc"), Some(&"beta".to_string()));
    assert_eq!(subtasks[1].envs.get("OTTO_FOREACH_INDEX"), Some(&"1".to_string()));
    assert!(subtasks[0].foreach.is_none(), "subtasks must not re-expand");
}

#[test]
fn test_expand_foreach_with_items_interpolates_input_and_output_per_item() {
    let mut task = TaskSpec::new(
        "build".to_string(),
        None,
        vec![],
        vec![],
        vec!["src/${item}.txt".to_string()],
        vec!["out/${item}.o".to_string()],
        HashMap::new(),
        ParamSpecs::new(),
        "echo ${item}".to_string(),
    );
    task.foreach = Some(command_foreach("unused: items are supplied"));
    task.foreach.as_mut().unwrap().var_name = "item".to_string();

    let items = vec![
        ForeachItem {
            identifier: "a".to_string(),
            value: "a".to_string(),
        },
        ForeachItem {
            identifier: "b".to_string(),
            value: "b".to_string(),
        },
    ];
    let subtasks = task.expand_foreach_with_items(&items).unwrap();

    assert_eq!(subtasks[0].input, vec!["src/a.txt".to_string()]);
    assert_eq!(subtasks[0].output, vec!["out/a.o".to_string()]);
    assert_eq!(subtasks[1].input, vec!["src/b.txt".to_string()]);
    assert_eq!(subtasks[1].output, vec!["out/b.o".to_string()]);
}

/// A non-loop variable in a foreach path is PRESERVED, not rejected.
///
/// **This test is deliberately inverted from the form Phase 6 shipped**, which
/// asserted `expand_foreach_with_items` errors on `${bogus}`. That behavior made
/// a foreach task reject exactly what a plain task accepts:
/// `examples/environment-variables/otto.yml` ships
/// `output: ["${BUILD_DIR}/${PROJECT_NAME}"]`, so global variables in paths are a
/// documented feature. Foreach expansion now resolves only its loop variable and
/// hands the rest to the environment pass in `Task::from_task_*`, which is where
/// the task's own `envs:` are finally merged and where a genuinely undefined
/// variable becomes an error. Found by the batched audit, batch 7 of 14.
#[test]
fn test_expand_foreach_with_items_preserves_a_non_loop_path_variable() {
    let mut task = TaskSpec::new(
        "build".to_string(),
        None,
        vec![],
        vec![],
        vec!["${SRCDIR}/${svc}.txt".to_string()],
        vec![],
        HashMap::new(),
        ParamSpecs::new(),
        "echo hi".to_string(),
    );
    task.foreach = Some(command_foreach("unused: items are supplied"));

    let items = vec![ForeachItem {
        identifier: "a".to_string(),
        value: "a".to_string(),
    }];
    let subtasks = task
        .expand_foreach_with_items(&items)
        .expect("a non-loop variable must survive");
    assert_eq!(
        subtasks[0].input,
        vec!["${SRCDIR}/a.txt".to_string()],
        "the loop variable resolves and the other reference is left for the env pass"
    );
}

#[test]
fn test_expand_foreach_with_items_rejects_duplicates() {
    let mut task = TaskSpec::new(
        "up".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        ParamSpecs::new(),
        "echo ${svc}".to_string(),
    );
    task.foreach = Some(command_foreach("printf 'a\na\n'"));

    let dup = ForeachItem {
        identifier: "a".to_string(),
        value: "a".to_string(),
    };
    let err = task
        .expand_foreach_with_items(&[dup.clone(), dup])
        .unwrap_err()
        .to_string();

    assert!(err.contains("duplicate subtask name 'up:a'"), "{err}");
}

#[test]
fn sanitize_identifier_keeps_an_item_to_one_path_component() {
    // The reproduced traversal: this identifier became the directory
    // `tasks/build:../../../ESCAPED`, i.e. a write outside the run's tasks/.
    assert_eq!(sanitize_identifier("../../../ESCAPED"), ".._.._.._ESCAPED");
    assert_eq!(sanitize_identifier("pkg/sub/name"), "pkg_sub_name");
    assert_eq!(sanitize_identifier(r"win\path"), "win_path");
    assert_eq!(sanitize_identifier("two words"), "two_words");
    assert_eq!(sanitize_identifier(".."), "_");
    assert_eq!(sanitize_identifier("."), "_");
    assert_eq!(sanitize_identifier(""), "_");
    // Ordinary identifiers are untouched.
    assert_eq!(sanitize_identifier("01-basic.sh"), "01-basic.sh");
    assert_eq!(sanitize_identifier("us-east-1"), "us-east-1");
}

#[test]
fn expand_foreach_cannot_name_a_subtask_outside_its_tasks_dir() {
    let mut task = TaskSpec::new(
        "build".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        ParamSpecs::new(),
        "echo ${pkg}".to_string(),
    );
    task.foreach = Some(command_foreach("printf '../../../ESCAPED\n'"));

    let items = vec![ForeachItem {
        identifier: "../../../ESCAPED".to_string(),
        value: "../../../ESCAPED".to_string(),
    }];
    let subtasks = task.expand_foreach_with_items(&items).unwrap();

    assert_eq!(subtasks[0].name, "build:.._.._.._ESCAPED");
    assert!(
        !subtasks[0].name.contains('/'),
        "a subtask name is one path component: {}",
        subtasks[0].name
    );
    // The value the script sees is untouched data.
    assert_eq!(
        subtasks[0].envs.get("OTTO_FOREACH_ITEM"),
        Some(&"../../../ESCAPED".to_string())
    );
}

#[test]
fn foreach_item_values_are_escaped_against_the_env_evaluator() {
    let mut task = TaskSpec::new(
        "build".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        ParamSpecs::new(),
        "echo ${pkg}".to_string(),
    );
    task.foreach = Some(command_foreach("printf 'a\n'"));

    let items = vec![ForeachItem {
        identifier: "item".to_string(),
        value: "${IFS}-$(touch /tmp/OTTO_PWNED)".to_string(),
    }];
    let subtasks = task.expand_foreach_with_items(&items).unwrap();

    // `$$` is the evaluator's literal-dollar escape; it evaluates back to the
    // item verbatim instead of aborting the task env with
    // `Environment variable 'IFS' not found` or running the command.
    let injected = subtasks[0].envs.get("OTTO_FOREACH_ITEM").unwrap();
    assert_eq!(injected, "$${IFS}-$$(touch /tmp/OTTO_PWNED)");

    let evaluated = crate::cfg::env::evaluate_envs(&subtasks[0].envs, None, &HashMap::new()).unwrap();
    assert_eq!(
        evaluated.get("OTTO_FOREACH_ITEM").map(String::as_str),
        Some("${IFS}-$(touch /tmp/OTTO_PWNED)")
    );
    assert!(
        !std::path::Path::new("/tmp/OTTO_PWNED").exists(),
        "the item must never have executed"
    );
}

#[test]
fn test_taskspec_expand_foreach_with_list() {
    let mut task = TaskSpec::new(
        "deploy".to_string(),
        Some("Deploy to environment".to_string()),
        vec![],
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        ParamSpecs::new(),
        "#!/bin/bash\necho deploy".to_string(),
    );
    task.foreach = Some(ForeachSpec {
        items: vec!["dev".to_string(), "staging".to_string(), "prod".to_string()],
        var_name: "env".to_string(),
        ..Default::default()
    });

    let cwd = PathBuf::from("/tmp");
    let subtasks = task.expand_foreach(&cwd).unwrap();

    assert_eq!(subtasks.len(), 3);
    assert_eq!(subtasks[0].name, "deploy:dev");
    assert_eq!(subtasks[1].name, "deploy:staging");
    assert_eq!(subtasks[2].name, "deploy:prod");

    // Check environment variables
    assert_eq!(subtasks[0].envs.get("env"), Some(&"dev".to_string()));
    assert_eq!(subtasks[0].envs.get("OTTO_FOREACH_ITEM"), Some(&"dev".to_string()));
    assert_eq!(subtasks[0].envs.get("OTTO_FOREACH_INDEX"), Some(&"0".to_string()));

    assert_eq!(subtasks[2].envs.get("OTTO_FOREACH_INDEX"), Some(&"2".to_string()));

    // Subtasks should not have foreach
    assert!(subtasks[0].foreach.is_none());
}

#[test]
fn test_taskspec_expand_foreach_none() {
    let task = TaskSpec::new(
        "build".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        ParamSpecs::new(),
        "#!/bin/bash\necho build".to_string(),
    );

    let cwd = PathBuf::from("/tmp");
    let subtasks = task.expand_foreach(&cwd).unwrap();

    assert_eq!(subtasks.len(), 1);
    assert_eq!(subtasks[0].name, "build");
}

#[test]
fn test_taskspec_as_virtual_parent() {
    let mut task = TaskSpec::new(
        "examples".to_string(),
        Some("Run examples".to_string()),
        vec![EdgeSpec::sugar("cleanup")],
        vec![EdgeSpec::sugar("build")],
        vec!["input.txt".to_string()],
        vec!["output.txt".to_string()],
        HashMap::from([("KEY".to_string(), "value".to_string())]),
        ParamSpecs::new(),
        "#!/bin/bash\necho hello".to_string(),
    );
    task.foreach = Some(ForeachSpec::default());

    let parent = task.as_virtual_parent();

    assert_eq!(parent.name, "examples");
    assert_eq!(parent.help, Some("Run examples".to_string()));
    assert_eq!(parent.after, vec![EdgeSpec::sugar("cleanup")]);
    assert_eq!(parent.before, vec![EdgeSpec::sugar("build")]);
    assert!(parent.input.is_empty());
    assert!(parent.output.is_empty());
    assert!(parent.envs.is_empty());
    assert!(parent.action.is_empty());
    assert!(parent.foreach.is_none());
    assert!(parent.virtual_parent);
}

/// Phase 4 negative test (design doc 2026-08-29, Phase 4 table). Parsed
/// through `ConfigSpec` so the error carries the full nesting path
/// (`tasks.up.foreach`).
#[test]
fn deny_unknown_fields_names_a_misspelled_foreach_items_key() {
    use crate::cfg::config::ConfigSpec;
    let yaml = "tasks:\n  up:\n    foreach:\n      itmes: [a, b]\n    bash: echo hi\n";
    let err = serde_yaml::from_str::<ConfigSpec>(yaml).unwrap_err().to_string();
    assert!(err.contains("itmes"), "must name the field: {err}");
    assert!(err.contains("tasks.up.foreach"), "must name the path: {err}");
}

/// Phase 4 negative test (design doc 2026-08-29, Phase 4 table). This is
/// the doc's motivating incident, verbatim: `parallel:` belongs under
/// `foreach:`, not beside it on the task. Before this phase it was
/// silently dropped and all subtasks ran concurrently; now it is a loud
/// config error naming the field and the path (`tasks.up`).
#[test]
fn deny_unknown_fields_names_a_wrong_level_parallel_key() {
    use crate::cfg::config::ConfigSpec;
    let yaml = "tasks:\n  up:\n    parallel: false\n    foreach: {items: [alpha, beta, gamma], as: svc}\n    bash: |\n      echo \"start ${svc}\"; sleep 0.3; echo \"end ${svc}\"\n";
    let err = serde_yaml::from_str::<ConfigSpec>(yaml).unwrap_err().to_string();
    assert!(err.contains("parallel"), "must name the field: {err}");
    assert!(err.contains("tasks.up"), "must name the path: {err}");
}

#[test]
fn test_foreach_yaml_deserialization() {
    let yaml = r#"
            help: "Run all examples"
            foreach:
              items: [a, b, c]
              as: example
              parallel: true
            bash: |
              echo ${example}
        "#;

    let task: TaskSpec = serde_yaml::from_str(yaml).unwrap();

    assert!(task.foreach.is_some());
    let foreach = task.foreach.unwrap();
    assert_eq!(foreach.items, vec!["a", "b", "c"]);
    assert_eq!(foreach.var_name, "example");
    assert!(foreach.parallel);
}

#[test]
fn test_foreach_yaml_deserialization_with_glob() {
    let yaml = r#"
            foreach:
              glob: "examples/*.sh"
            bash: echo test
        "#;

    let task: TaskSpec = serde_yaml::from_str(yaml).unwrap();

    assert!(task.foreach.is_some());
    let foreach = task.foreach.unwrap();
    assert_eq!(foreach.glob, Some("examples/*.sh".to_string()));
    assert_eq!(foreach.var_name, "item"); // default
}

#[test]
fn test_foreach_yaml_deserialization_with_range() {
    let yaml = r#"
            foreach:
              range: "1-10"
              as: num
            bash: echo ${num}
        "#;

    let task: TaskSpec = serde_yaml::from_str(yaml).unwrap();

    assert!(task.foreach.is_some());
    let foreach = task.foreach.unwrap();
    assert_eq!(foreach.range, Some("1-10".to_string()));
    assert_eq!(foreach.var_name, "num");
}

// ------------------------------------------------------------------
// Phase 7: tty: true
// ------------------------------------------------------------------

#[test]
fn test_tty_defaults_to_none_when_absent() {
    let yaml = "action: echo hi";
    let spec: TaskSpec = serde_yaml::from_str(yaml).expect("parse failed");
    assert_eq!(spec.tty, None, "an ottofile without tty: must not gain one");
}

#[test]
fn test_tty_parses_both_values() {
    let on: TaskSpec = serde_yaml::from_str("action: aws sso login\ntty: true").expect("parse failed");
    assert_eq!(on.tty, Some(true));
    let off: TaskSpec = serde_yaml::from_str("action: echo hi\ntty: false").expect("parse failed");
    assert_eq!(off.tty, Some(false));
}

#[test]
fn test_tty_serializes_only_when_set() {
    let mut spec = TaskSpec::new(
        "login".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        ParamSpecs::new(),
        "#!/bin/bash\naws sso login".to_string(),
    );
    assert!(!serde_yaml::to_string(&spec).unwrap().contains("tty"));
    spec.tty = Some(true);
    assert!(serde_yaml::to_string(&spec).unwrap().contains("tty: true"));
}

/// A task naming both `bash:` and `python:` used to silently run bash and
/// drop python with no warning. Now it is a loud config-load error naming
/// every source present.
#[test]
fn bash_and_python_together_is_a_loud_config_error() {
    let yaml = "bash: echo FROM_BASH\npython: print('FROM_PYTHON')\n";
    let err = serde_yaml::from_str::<TaskSpec>(yaml).unwrap_err().to_string();
    assert!(err.contains("bash"), "{err}");
    assert!(err.contains("python"), "{err}");
}

/// Dedent used to compute indentation in bytes (two ascii spaces = 2
/// bytes) while slicing every line at that same byte offset, including a
/// sibling line indented by one U+2002 (EN SPACE, 3 bytes): byte offset 2
/// lands mid-character there and panicked, "byte index 2 is not a char
/// boundary; it is inside '\u{2002}'". Char-counted indent sidesteps it.
#[test]
fn deserialize_script_string_dedents_multibyte_whitespace_without_panicking() {
    let script = "  a\n\u{2002}b";
    assert_eq!(deserialize_script_string(script), "a\nb");
}

/// The same, across every multibyte whitespace character likely to reach a
/// script body, plus a mixed multibyte/ASCII indent.
///
/// The shipped case pinned U+2002 alone, which is the character that produced
/// the original panic - but the defect was byte-vs-char slicing, so any
/// multibyte indent reaches it. Widened by the batched audit, batch 7 of 14.
#[test]
fn deserialize_script_string_dedents_every_multibyte_whitespace_width() {
    // (label, the character, its UTF-8 length in bytes)
    let widths = [
        ("U+00A0 NO-BREAK SPACE", '\u{00A0}', 2),
        ("U+2002 EN SPACE", '\u{2002}', 3),
        ("U+2003 EM SPACE", '\u{2003}', 3),
        ("U+2009 THIN SPACE", '\u{2009}', 3),
        ("U+3000 IDEOGRAPHIC SPACE", '\u{3000}', 3),
    ];

    for (label, ch, want_len) in widths {
        assert_eq!(ch.len_utf8(), want_len, "{label}: fixture assumption wrong");
        let script = format!("  a\n{ch}b");
        assert_eq!(
            deserialize_script_string(&script),
            "a\nb",
            "{label} panicked or mis-dedented"
        );

        // Mixed indent: a multibyte character followed by ASCII, so the byte
        // offset and the char offset disagree by more than one. Both lines carry
        // a two-CHARACTER indent, so both dedent fully - which is the point: the
        // strip is counted in characters, not bytes.
        let mixed = format!("  a\n{ch} b");
        assert_eq!(deserialize_script_string(&mixed), "a\nb", "{label} mixed indent");
    }
}

/// `tty:` on a foreach task means "give each of these the terminal", so every
/// generated subtask carries it. The exclusivity gate then serializes them
/// even under `parallel: true`; that is documented behavior, not an error.
#[test]
fn test_foreach_subtasks_inherit_tty() {
    let mut task = TaskSpec::new(
        "login".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        ParamSpecs::new(),
        "#!/bin/bash\necho ${item}".to_string(),
    );
    task.tty = Some(true);
    task.foreach = Some(ForeachSpec {
        items: vec!["a".to_string(), "b".to_string()],
        ..Default::default()
    });

    let subtasks = task.expand_foreach(Path::new(".")).expect("expansion failed");

    assert_eq!(subtasks.len(), 2);
    for subtask in &subtasks {
        assert_eq!(subtask.tty, Some(true), "{} lost tty", subtask.name);
    }
}

/// The virtual parent runs no script, so it must not claim the terminal - it
/// would take the exclusive permit to do nothing.
#[test]
fn test_virtual_parent_drops_tty() {
    let mut task = TaskSpec::new(
        "login".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        ParamSpecs::new(),
        "#!/bin/bash\necho hi".to_string(),
    );
    task.tty = Some(true);

    assert_eq!(task.as_virtual_parent().tty, None);
}

// ========================================================================
// Phase 5 drift test (design doc 2026-08-29, Phase 5(b) / Resolved
// Decisions "panel round 2"): docs/commands/ottofile-reference.md must
// name every key of every deny_unknown_fields struct, PLUS EdgeSpec
// (src/cfg/edge.rs), whose hand-written `visit_map` enforces the same
// "unknown field" contract without the derive macro (round-4 audit
// cheap-win: EdgeSpec's `task`/`when` keys had no reference-doc rows and
// tripped none of the checks below). Two zero-new-crate techniques, used
// together, so the reference cannot drift silently:
//
// 1. Exhaustive destructuring below is the compile-time TRIGGER: if any
//    of the seven structs gains or loses a field, the destructuring
//    pattern stops matching and the BUILD breaks, before any test even
//    runs. This is what reaches private `TaskSpecHelper` from inside this
//    file's own `#[cfg(test)]` module.
// 2. `expected_keys_from_deny_unknown_fields` recovers each struct's real
//    on-disk key list (renames already applied, e.g. `as` not `var_name`)
//    straight out of serde's own "unknown field" error message, by
//    feeding it a single bogus key. Works identically against EdgeSpec's
//    hand-written `visit_map`: it returns `Error::unknown_field(other,
//    &["task", "when"])`, which stringifies to the exact same "unknown
//    field `x`, expected `task` or `when`" shape serde's derive macro
//    produces for a two-field struct (verified directly: no location
//    suffix, same phrasing). Nothing here hand-copies a key list that
//    could go stale on its own.
// ========================================================================

use crate::cfg::config::ConfigSpec;
use crate::cfg::otto::{OttoSpec, RetentionSpec};
use crate::cfg::param::ParamSpec;

/// Every field on the six `deny_unknown_fields` structs has a default (per
/// the design doc's Data Model), so a document containing nothing but one
/// bogus key is enough to provoke the "unknown field" error and never a
/// "missing field" one instead.
const BOGUS_KEY_PROBE: &str = "__ottofile_reference_drift_probe__: true\n";

/// Parse `T` from [`BOGUS_KEY_PROBE`] and read its real on-disk field names
/// back out of the `deny_unknown_fields` error text, rather than hand-
/// copying them. Handles all three of serde's phrasings: "expected `a`",
/// "expected `a` or `b`", and "expected one of `a`, `b`, ..., `z`".
fn expected_keys_from_deny_unknown_fields<T: serde::de::DeserializeOwned>() -> Vec<String> {
    let err = serde_yaml::from_str::<T>(BOGUS_KEY_PROBE)
        .err()
        .expect("bogus key must be rejected")
        .to_string();
    let after_expected = err
        .split("expected")
        .nth(1)
        .unwrap_or_else(|| panic!("deny_unknown_fields error names no expected set: {err}"));
    // A location suffix (" at line N column M") is present except for the
    // root ConfigSpec case, which has no parent path (Phase 4 table).
    let before_location = after_expected.split(" at line").next().unwrap();
    before_location
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// True when `expected_path` appears in the reference doc as the ENTIRE
/// value of some backtick-quoted span (e.g. `otto.retention.keep_days`
/// matches the doc's `` `otto.retention.keep_days` `` row exactly).
///
/// This is a full-path match, not a trailing-dot-segment match. Three
/// on-disk key names are reused at more than one level (`tasks`: root
/// `ConfigSpec.tasks` vs. `otto.tasks`; `envs`: `otto.envs` vs.
/// `tasks.<name>.envs`; `help`: `tasks.<name>.help` vs.
/// `...params.<title>.help`), so a bare trailing-segment match (matching
/// `tasks` against ANY span ending in `.tasks`) would let one level's row
/// silently vouch for a different level's key, going undetected if that
/// key's own row were deleted. Requiring the full path per level closes
/// that hole (round-4 implementation audit, cheap-win 3).
fn reference_doc_mentions_key_at(doc: &str, expected_path: &str) -> bool {
    doc.split('`').skip(1).step_by(2).any(|token| token == expected_path)
}

/// Doc-path builders, one per struct, mirroring the reference doc's own
/// level headings (`## otto:`, `## otto.retention:`, `## tasks.<name>:`,
/// `## tasks.<name>.foreach:`, `## tasks.<name>.params.<title>:`) plus one
/// for `EdgeSpec`'s `tasks.<name>.after[]`/`before[]` object. Applying the
/// right prefix to a recovered on-disk key name is what lets the exact
/// match above tell same-named keys at different levels apart.
fn root_path(key: &str) -> String {
    key.to_string()
}
fn otto_path(key: &str) -> String {
    format!("otto.{key}")
}
fn retention_path(key: &str) -> String {
    format!("otto.retention.{key}")
}
fn task_path(key: &str) -> String {
    format!("tasks.<name>.{key}")
}
fn foreach_path(key: &str) -> String {
    format!("tasks.<name>.foreach.{key}")
}
fn param_path(key: &str) -> String {
    format!("...params.<title>.{key}")
}
fn edge_path(key: &str) -> String {
    format!("tasks.<name>.after[].{key}")
}

#[test]
fn ottofile_reference_key_inventory_is_exhaustive() {
    // --- Compile-time trigger: exhaustive destructuring of all six ---
    // Every field is bound to `_`; the build breaks the moment any struct
    // gains or loses a field, forcing whoever changed the schema to touch
    // this test (and, from there, the reference doc) immediately.
    let ConfigSpec { otto: _, tasks: _ } = ConfigSpec::default();
    let OttoSpec {
        name: _,
        about: _,
        api: _,
        jobs: _,
        tasks: _,
        envs: _,
        envs_command: _,
        retention: _,
    } = OttoSpec::default();
    let RetentionSpec {
        keep_days: _,
        keep_last: _,
        keep_failed: _,
        auto_prune: _,
        prune_interval_hours: _,
    } = RetentionSpec::default();
    let ForeachSpec {
        glob: _,
        items: _,
        range: _,
        command: _,
        var_name: _,
        parallel: _,
        max_items: _,
    } = ForeachSpec::default();
    let TaskSpecHelper {
        help: _,
        after: _,
        before: _,
        input: _,
        output: _,
        envs: _,
        params: _,
        bash: _,
        python: _,
        action: _,
        foreach: _,
        on_failure: _,
        tty: _,
    } = serde_yaml::from_str::<TaskSpecHelper>("{}\n").unwrap();
    let ParamSpec {
        name: _,
        short: _,
        long: _,
        param_type: _,
        metavar: _,
        default: _,
        choices: _,
        choices_command: _,
        nargs: _,
        help: _,
        required: _,
        value: _,
    } = serde_yaml::from_str::<ParamSpec>("{}\n").unwrap();
    // EdgeSpec has two bookkeeping fields (`from_sugar`, `is_injected_sugar`)
    // alongside its two on-disk keys (`task`, `when`); all four are bound
    // here so the destructuring still breaks on ANY field added or removed,
    // but only `task`/`when` are on-disk keys checked against the doc below.
    let EdgeSpec {
        task: _,
        when: _,
        from_sugar: _,
        is_injected_sugar: _,
    } = EdgeSpec::sugar("probe");

    // --- Runtime check: recovered on-disk keys vs. the reference doc ---
    let doc = include_str!("../../docs/commands/ottofile-reference.md");

    type PathBuilder = fn(&str) -> String;
    type KeyProbe = (&'static str, usize, fn() -> Vec<String>, PathBuilder);
    let expectations: &[KeyProbe] = &[
        (
            "ConfigSpec",
            2,
            expected_keys_from_deny_unknown_fields::<ConfigSpec>,
            root_path,
        ),
        (
            "OttoSpec",
            8,
            expected_keys_from_deny_unknown_fields::<OttoSpec>,
            otto_path,
        ),
        (
            "RetentionSpec",
            5,
            expected_keys_from_deny_unknown_fields::<RetentionSpec>,
            retention_path,
        ),
        (
            "ForeachSpec",
            7,
            expected_keys_from_deny_unknown_fields::<ForeachSpec>,
            foreach_path,
        ),
        (
            "TaskSpecHelper",
            13,
            expected_keys_from_deny_unknown_fields::<TaskSpecHelper>,
            task_path,
        ),
        (
            "ParamSpec",
            7,
            expected_keys_from_deny_unknown_fields::<ParamSpec>,
            param_path,
        ),
        (
            "EdgeSpec",
            2,
            expected_keys_from_deny_unknown_fields::<EdgeSpec>,
            edge_path,
        ),
    ];

    let mut total = 0;
    for (struct_name, expected_count, probe, path_for) in expectations {
        let keys = probe();
        assert_eq!(
            keys.len(),
            *expected_count,
            "{struct_name}: expected {expected_count} on-disk keys, serde reports {}: {keys:?}",
            keys.len()
        );
        for key in &keys {
            let expected_path = path_for(key);
            assert!(
                reference_doc_mentions_key_at(doc, &expected_path),
                "{struct_name}'s on-disk key `{key}` (expected doc path \
                 `{expected_path}`) is not mentioned in \
                 docs/commands/ottofile-reference.md"
            );
        }
        total += keys.len();
    }
    assert_eq!(
        total, 44,
        "total on-disk key count drifted from the design doc's count of 44"
    );

    // The per-key loop above only proves each key *name* appears somewhere on
    // the page, so it cannot see a wrong total: the reference doc said "Total:
    // 44" and "`OttoSpec` 9" for a full day after the schema moved to 42/7,
    // contradicting its own section header twenty lines earlier. Pin the prose
    // arithmetic too.
    let stated_total = format!("## Total: {total} fixed keys");
    assert!(
        doc.contains(&stated_total),
        "docs/commands/ottofile-reference.md must state `{stated_total}`; its total section is stale"
    );
    assert!(
        doc.contains(&format!("= **{total}**")),
        "docs/commands/ottofile-reference.md's per-struct sum must total **{total}**"
    );
    assert!(
        doc.contains(&format!("every one of those {total}")),
        "docs/commands/ottofile-reference.md's drift-test description must name {total} recovered keys"
    );
    for (struct_name, expected_count, _, _) in expectations {
        assert!(
            doc.contains(&format!("`{struct_name}` {expected_count}")),
            "docs/commands/ottofile-reference.md's per-struct sum must read `{struct_name}` {expected_count}"
        );
    }
}
