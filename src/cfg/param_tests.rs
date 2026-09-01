#![cfg(test)]

use super::*;
use std::collections::HashMap;
use tempfile::TempDir;

#[test]
fn test_boolean_flag_detection_true_default() {
    use crate::cfg::task::TaskSpec;

    let yaml = r#"
        params:
          -v|--verbose:
            default: true
            help: Enable verbose output
        "#;

    let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
    let verbose = task_spec.params.get("verbose").unwrap();

    assert_eq!(verbose.param_type, ParamType::FLG);
    assert_eq!(verbose.short, Some('v'));
    assert_eq!(verbose.long, Some("verbose".to_string()));
    assert_eq!(verbose.default, Some("true".to_string()));
    assert_eq!(verbose.name, "verbose");
}

#[test]
fn test_boolean_flag_detection_false_default() {
    use crate::cfg::task::TaskSpec;

    let yaml = r#"
        params:
          --debug:
            default: false
            help: Enable debug mode
        "#;

    let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
    let debug = task_spec.params.get("debug").unwrap();

    assert_eq!(debug.param_type, ParamType::FLG);
    assert_eq!(debug.short, None);
    assert_eq!(debug.long, Some("debug".to_string()));
    assert_eq!(debug.default, Some("false".to_string()));
    assert_eq!(debug.name, "debug");
}

#[test]
fn test_boolean_flag_case_insensitive() {
    use crate::cfg::task::TaskSpec;

    let yaml = r#"
        params:
          --enable:
            default: TRUE
            help: Enable feature
        "#;

    let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
    let enable = task_spec.params.get("enable").unwrap();

    assert_eq!(enable.param_type, ParamType::FLG);
    assert_eq!(enable.default, Some("TRUE".to_string()));
}

#[test]
fn test_argument_flag_with_choices() {
    use crate::cfg::task::TaskSpec;

    let yaml = r#"
        params:
          -e|--env:
            default: development
            choices: [development, staging, production]
            help: Target environment
        "#;

    let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
    let env = task_spec.params.get("env").unwrap();

    assert_eq!(env.param_type, ParamType::OPT);
    assert_eq!(env.short, Some('e'));
    assert_eq!(env.long, Some("env".to_string()));
    assert_eq!(env.choices, vec!["development", "staging", "production"]);
    assert_eq!(env.default, Some("development".to_string()));
    assert_eq!(env.name, "env");
}

#[test]
fn test_argument_flag_no_default() {
    use crate::cfg::task::TaskSpec;

    let yaml = r#"
        params:
          -c|--config:
            help: Path to config file
        "#;

    let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
    let config = task_spec.params.get("config").unwrap();

    assert_eq!(config.param_type, ParamType::OPT);
    assert_eq!(config.short, Some('c'));
    assert_eq!(config.long, Some("config".to_string()));
    assert_eq!(config.default, None);
    assert!(config.choices.is_empty());
}

#[test]
fn test_positional_parameter() {
    use crate::cfg::task::TaskSpec;

    let yaml = r#"
        params:
          filename:
            help: Input filename
        "#;

    let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
    let filename = task_spec.params.get("filename").unwrap();

    assert_eq!(filename.param_type, ParamType::POS);
    assert_eq!(filename.short, None);
    assert_eq!(filename.long, None);
    assert_eq!(filename.name, "filename");
}

#[test]
fn test_positional_parameter_with_metavar() {
    use crate::cfg::task::TaskSpec;

    let yaml = r#"
        params:
          input_file:
            help: Input file path
            metavar: FILE
        "#;

    let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
    let input_file = task_spec.params.get("input_file").unwrap();

    assert_eq!(input_file.param_type, ParamType::POS);
    assert_eq!(input_file.metavar, Some("FILE".to_string()));
}

#[test]
fn test_mixed_parameters() {
    use crate::cfg::task::TaskSpec;

    let yaml = r#"
        params:
          -v|--verbose:
            default: false
            help: Enable verbose output
          -e|--env:
            default: development
            choices: [development, staging, production]
            help: Target environment
          --timeout:
            default: 30
            help: Timeout in seconds
          input_file:
            help: Input file path
        "#;

    let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();

    // Boolean flag
    let verbose = task_spec.params.get("verbose").unwrap();
    assert_eq!(verbose.param_type, ParamType::FLG);
    assert_eq!(verbose.short, Some('v'));
    assert_eq!(verbose.long, Some("verbose".to_string()));

    // Argument flag with choices
    let env = task_spec.params.get("env").unwrap();
    assert_eq!(env.param_type, ParamType::OPT);
    assert_eq!(env.choices.len(), 3);

    // Argument flag without choices
    let timeout = task_spec.params.get("timeout").unwrap();
    assert_eq!(timeout.param_type, ParamType::OPT);
    assert!(timeout.choices.is_empty());

    // Positional parameter
    let input_file = task_spec.params.get("input_file").unwrap();
    assert_eq!(input_file.param_type, ParamType::POS);
}

/// Zero `skip_serializing_if` in `src/cfg/` meant a minimal param (only
/// `help:` set) re-emitted every other field as an explicit null/empty
/// value: `metavar: null, default: null, choices: [], choices-command:
/// null, nargs: '1'`. None of those should appear when unset.
#[test]
fn a_minimal_param_serializes_with_no_null_valued_keys() {
    use crate::cfg::task::TaskSpec;
    let yaml = "params:\n  filename:\n    help: Input file\nbash: echo hi\n";
    let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
    let emitted = serde_yaml::to_string(&task_spec).unwrap();
    for absent in ["metavar", "default", "choices-command", "nargs"] {
        assert!(!emitted.contains(absent), "must not emit unset `{absent}`:\n{emitted}");
    }
    assert!(
        !emitted.contains("choices: []"),
        "must not emit an empty choices list:\n{emitted}"
    );
    assert!(
        emitted.contains("help: Input file"),
        "must still emit the set field:\n{emitted}"
    );
}

/// Documents the contract that rich_key() is a *pure function of the
/// current ParamSpec fields*, not a cached value of the input key. If a
/// future refactor caches divine()'s input on the struct, this test
/// breaks — that's the signal that the divine/rich_key inverse-pair
/// invariant has been compromised and Option D (see design doc) should
/// be revisited.
#[test]
fn rich_key_reflects_current_fields() {
    let mut spec = ParamSpec {
        name: "verbose".to_string(),
        short: Some('v'),
        long: Some("verbose".to_string()),
        param_type: ParamType::FLG,
        metavar: None,
        default: Some("false".to_string()),
        choices_command: None,
        choices: vec![],
        nargs: Nargs::default(),
        help: None,
        value: Value::Empty,
        required: false,
    };
    assert_eq!(rich_key(&spec), "-v|--verbose");

    spec.long = Some("rename".to_string());
    assert_eq!(rich_key(&spec), "-v|--rename");

    spec.short = None;
    assert_eq!(rich_key(&spec), "--rename");

    spec.long = None;
    spec.name = "positional".to_string();
    assert_eq!(rich_key(&spec), "positional");
}

#[test]
fn test_divine_function_short_only() {
    let (name, short, long) = divine("-v").unwrap();
    assert_eq!(name, "v");
    assert_eq!(short, Some('v'));
    assert_eq!(long, None);
}

#[test]
fn test_divine_function_long_only() {
    let (name, short, long) = divine("--verbose").unwrap();
    assert_eq!(name, "verbose");
    assert_eq!(short, None);
    assert_eq!(long, Some("verbose".to_string()));
}

#[test]
fn test_divine_function_both() {
    let (name, short, long) = divine("-v|--verbose").unwrap();
    assert_eq!(name, "verbose");
    assert_eq!(short, Some('v'));
    assert_eq!(long, Some("verbose".to_string()));
}

#[test]
fn test_divine_function_reverse_order() {
    let (name, short, long) = divine("--verbose|-v").unwrap();
    assert_eq!(name, "verbose");
    assert_eq!(short, Some('v'));
    assert_eq!(long, Some("verbose".to_string()));
}

#[test]
fn test_divine_function_no_flags() {
    let (name, short, long) = divine("filename").unwrap();
    assert_eq!(name, "filename");
    assert_eq!(short, None);
    assert_eq!(long, None);
}

/// `-v|-x` used to silently keep only the first short and drop `-x`.
#[test]
fn divine_rejects_two_short_flags() {
    let err = divine("-v|-x").unwrap_err().to_string();
    assert!(err.contains("-v|-x"), "{err}");
}

/// `--foo|--bar` used to concatenate into a bogus single long `"foo--bar"`.
#[test]
fn divine_rejects_two_long_flags() {
    let err = divine("--foo|--bar").unwrap_err().to_string();
    assert!(err.contains("--foo|--bar"), "{err}");
}

/// `-verbose` (one dash, multiple letters) used to become a *positional*
/// param literally named `-verbose`, almost certainly a typo for
/// `--verbose`.
#[test]
fn divine_rejects_a_single_dash_multi_char_flag() {
    let err = divine("-verbose").unwrap_err().to_string();
    assert!(err.contains("-verbose"), "{err}");
}

/// A bare name cannot be mixed with a flag form in the same key.
#[test]
fn divine_rejects_a_bare_name_mixed_with_a_flag() {
    let err = divine("filename|-x").unwrap_err().to_string();
    assert!(err.contains("filename|-x"), "{err}");
}

/// `-v|--verbose` plus a separate `--verbose` key in one params map used
/// to silently collapse to one entry (last-wins), losing the first with
/// no signal it ever existed.
#[test]
fn deny_duplicate_divined_names_in_one_params_map() {
    use crate::cfg::task::TaskSpec;
    let yaml = r#"
        params:
          -v|--verbose:
            help: first
          --verbose:
            help: second
        "#;
    let err = serde_yaml::from_str::<TaskSpec>(yaml).unwrap_err().to_string();
    assert!(err.contains("verbose"), "{err}");
}

/// Phase 4 negative test (design doc 2026-08-29, Phase 4 table). `name:`
/// under a param is a `#[serde(skip)]` field, derived from the params-map
/// key by `divine()`, never a user-writable key. Before this phase it was
/// a silent no-op; now it is REJECTED input, naming the field and the
/// path down to the rich param key.
#[test]
fn deny_unknown_fields_rejects_a_skip_field_written_by_the_user() {
    use crate::cfg::config::ConfigSpec;
    let yaml = "tasks:\n  up:\n    params:\n      -s|--svc:\n        name: something\n    bash: echo hi\n";
    let err = serde_yaml::from_str::<ConfigSpec>(yaml).unwrap_err().to_string();
    assert!(err.contains("name"), "must name the field: {err}");
    assert!(err.contains("tasks.up.params"), "must name the path: {err}");
    assert!(err.contains("-s|--svc"), "must name the rich param key: {err}");
}

#[test]
fn test_value_display() {
    assert_eq!(Value::Empty.to_string(), "Value::Empty");
    assert_eq!(Value::Item("test".to_string()).to_string(), "Value::Item(test)");
    assert_eq!(
        Value::List(vec!["a".to_string(), "b".to_string()]).to_string(),
        "Value::List([a, b])"
    );

    let mut dict = HashMap::new();
    dict.insert("key".to_string(), "value".to_string());
    assert_eq!(Value::Dict(dict).to_string(), "Value::Dict({key: value})");
}

/// `nargs: "0:5"` used to panic (`attempt to subtract with overflow` at
/// `min - 1`); now `min` of 0 is rejected outright, naming the param via
/// the surrounding ConfigSpec's yaml path.
#[test]
fn nargs_zero_min_is_rejected_naming_the_param() {
    use crate::cfg::config::ConfigSpec;
    let yaml = "tasks:\n  build:\n    params:\n      -v|--verbose:\n        nargs: \"0:5\"\n    bash: echo hi\n";
    let err = serde_yaml::from_str::<ConfigSpec>(yaml).unwrap_err().to_string();
    assert!(err.contains("-v|--verbose"), "{err}");
    assert!(err.contains("0:5"), "{err}");
}

/// `"5:2"` used to silently accept an inverted range (`Range(4, 2)`).
#[test]
fn nargs_inverted_range_is_rejected_naming_the_param() {
    use crate::cfg::config::ConfigSpec;
    let yaml = "tasks:\n  build:\n    params:\n      --files:\n        nargs: \"5:2\"\n    bash: echo hi\n";
    let err = serde_yaml::from_str::<ConfigSpec>(yaml).unwrap_err().to_string();
    assert!(err.contains("files"), "{err}");
    assert!(err.contains("5:2"), "{err}");
}

/// `"1:2:3"` used to silently drop the third part (`Range(0, 2)`).
#[test]
fn nargs_extra_colon_is_rejected_naming_the_param() {
    use crate::cfg::config::ConfigSpec;
    let yaml = "tasks:\n  build:\n    params:\n      --files:\n        nargs: \"1:2:3\"\n    bash: echo hi\n";
    let err = serde_yaml::from_str::<ConfigSpec>(yaml).unwrap_err().to_string();
    assert!(err.contains("files"), "{err}");
    assert!(err.contains("1:2:3"), "{err}");
}

/// `nargs: ""` errors, but only incidentally: it falls through to the bare
/// bare-number branch and fails `"".parse::<usize>()`, not a dedicated
/// emptiness check. Pinned so a future refactor of that branch can't turn
/// this into a panic or a silent default without a test going red.
#[test]
fn nargs_empty_string_is_rejected() {
    use crate::cfg::config::ConfigSpec;
    let yaml = "tasks:\n  build:\n    params:\n      --files:\n        nargs: \"\"\n    bash: echo hi\n";
    let err = serde_yaml::from_str::<ConfigSpec>(yaml).unwrap_err().to_string();
    assert!(err.contains("files"), "{err}");
    assert!(err.contains("invalid count"), "{err}");
}

#[test]
fn nargs_valid_range_still_parses() {
    let nargs: Nargs = serde_yaml::from_str("\"2:5\"").unwrap();
    assert_eq!(nargs, Nargs::Range(1, 5));
}

#[test]
fn test_nargs_roundtrip_all_variants() {
    let cases = vec![
        Nargs::One,
        Nargs::Zero,
        Nargs::OneOrZero,
        Nargs::OneOrMore,
        Nargs::ZeroOrMore,
        Nargs::Range(0, 3),
        Nargs::Range(2, 5),
    ];
    for nargs in cases {
        let yaml = serde_yaml::to_string(&nargs).unwrap();
        let parsed: Nargs = serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("failed to parse {yaml:?}: {e}"));
        assert_eq!(nargs, parsed, "round-trip failed for {nargs:?} (yaml: {yaml:?})");
    }
}

// =========================================================================
// choices-command (design doc Phase 6b)
// =========================================================================

fn spec_for(yaml: &str, param: &str) -> ParamSpec {
    use crate::cfg::task::TaskSpec;
    let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
    task_spec.params.get(param).unwrap().clone()
}

#[test]
fn choices_command_parses_from_the_kebab_case_key() {
    let spec = spec_for(
        r#"
            params:
              -s|--svc:
                choices-command: "printf 'alpha\nbeta\n'"
                help: Service to switch to
            "#,
        "svc",
    );
    assert_eq!(spec.choices_command.as_deref(), Some("printf 'alpha\nbeta\n'"));
    assert!(spec.has_dynamic_choices());
    assert!(spec.choices.is_empty());
}

#[test]
fn choices_command_serializes_back_to_the_kebab_case_key() {
    // A deserialize-only rename would emit `choices_command` here, and the
    // ottofile would silently lose the field on the next parse.
    let spec = spec_for(
        r#"
            params:
              -s|--svc:
                choices-command: "list-services"
            "#,
        "svc",
    );
    let emitted = serde_yaml::to_string(&spec).unwrap();
    assert!(emitted.contains("choices-command: list-services"), "{emitted}");
    assert!(!emitted.contains("choices_command"), "{emitted}");
}

#[test]
fn a_param_without_choices_command_stays_static() {
    let spec = spec_for(
        r#"
            params:
              -e|--env:
                choices: [dev, prod]
            "#,
        "env",
    );
    assert!(!spec.has_dynamic_choices());
    assert_eq!(spec.choices, vec!["dev".to_string(), "prod".to_string()]);
}

#[test]
fn choices_and_choices_command_together_is_a_loud_config_error() {
    use crate::cfg::task::TaskSpec;
    let err = serde_yaml::from_str::<TaskSpec>(
        r#"
            params:
              -s|--svc:
                choices: [alpha, beta]
                choices-command: "list-services"
            "#,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("svc"), "must name the param: {err}");
    assert!(err.contains("list-services"), "must name the command: {err}");
    assert!(err.contains("alpha, beta"), "must name the static choices: {err}");
}

#[test]
fn resolve_choices_command_returns_trimmed_non_empty_lines() {
    let spec = spec_for(
        r#"
            params:
              -s|--svc:
                choices-command: "printf 'alpha\n\n  beta  \n'"
            "#,
        "svc",
    );
    let cwd = TempDir::new().unwrap();
    let values = spec
        .resolve_choices_command("switch", cwd.path(), &HashMap::new())
        .unwrap();
    assert_eq!(values, vec!["alpha".to_string(), "beta".to_string()]);
}

#[test]
fn resolve_choices_command_nonzero_exit_names_task_param_and_command() {
    let spec = spec_for(
        r#"
            params:
              -s|--svc:
                choices-command: "echo nope >&2; exit 4"
            "#,
        "svc",
    );
    let cwd = TempDir::new().unwrap();
    let err = spec
        .resolve_choices_command("switch", cwd.path(), &HashMap::new())
        .unwrap_err()
        .to_string();
    assert!(err.contains("switch"), "{err}");
    assert!(err.contains("svc"), "{err}");
    assert!(err.contains("echo nope >&2; exit 4"), "{err}");
    assert!(err.contains("exit code 4"), "{err}");
}

#[test]
fn resolve_choices_command_zero_lines_is_an_error_unlike_foreach() {
    let spec = spec_for(
        r#"
            params:
              -s|--svc:
                choices-command: "true"
            "#,
        "svc",
    );
    let cwd = TempDir::new().unwrap();
    let err = spec
        .resolve_choices_command("switch", cwd.path(), &HashMap::new())
        .unwrap_err()
        .to_string();
    assert!(err.contains("switch"), "{err}");
    assert!(err.contains("svc"), "{err}");
    assert!(err.contains("no values"), "{err}");
}
