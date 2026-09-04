#![cfg(test)]

use super::*;

// =========================================================================
// unknown-task detection (Phase 1: silent-success criticals)
// =========================================================================

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_unconsumed_args_none_when_first_arg_is_a_task() {
    let args = names(&["build", "--flag", "value"]);
    assert_eq!(unconsumed_args(&args, &names(&["build", "test"])), None);
}

#[test]
fn test_unconsumed_args_reports_leading_unknown_names() {
    let args = names(&["nonexistent", "build"]);
    let unknown = unconsumed_args(&args, &names(&["build", "test"])).expect("the leading arg is unknown");
    assert_eq!(unknown, ["nonexistent".to_string()]);
}

#[test]
fn test_unconsumed_args_reports_everything_when_no_arg_names_a_task() {
    let args = names(&["nope", "alsonope"]);
    let unknown = unconsumed_args(&args, &names(&["build"])).expect("nothing was consumed");
    assert_eq!(unknown, ["nope".to_string(), "alsonope".to_string()]);
}

#[test]
fn test_unconsumed_args_empty_input() {
    assert_eq!(unconsumed_args(&[], &names(&["build"])), None);
}

#[test]
fn test_nearest_task_name_finds_a_one_edit_typo() {
    assert_eq!(nearest_task_name("buld", &names(&["build", "test"])), Some("build"));
}

#[test]
fn test_nearest_task_name_gives_up_on_a_distant_name() {
    assert_eq!(nearest_task_name("deploy-everything", &names(&["build", "test"])), None);
}

#[test]
fn test_nearest_task_name_ignores_help() {
    // `help` is a partition-only entry, not a task to suggest. The lowercase
    // `"graph"` this test used to pair it with was never in a production name
    // list: the builtin is `Graph`.
    assert_eq!(nearest_task_name("hel", &names(&["help"])), None);
}

#[test]
fn test_unknown_task_error_carries_a_suggestion() {
    let err = unknown_task_error(&names(&["buld"]), &names(&["build"]));
    let msg = err.to_string();
    assert!(msg.contains("unknown task 'buld'"), "{msg}");
    assert!(msg.contains("did you mean 'build'?"), "{msg}");
}

#[test]
fn test_unknown_task_error_without_a_near_match() {
    let err = unknown_task_error(&names(&["zzzzzzzz"]), &names(&["build"]));
    let msg = err.to_string();
    assert_eq!(msg, "unknown task 'zzzzzzzz'");
}

#[test]
fn test_unknown_task_error_lists_every_unconsumed_name() {
    let err = unknown_task_error(&names(&["zzzzzzzz", "yyyyyyyy"]), &names(&["build"]));
    assert_eq!(err.to_string(), "unknown tasks: zzzzzzzz, yyyyyyyy");
}

#[test]
fn test_parse_outcome_into_run_rejects_an_exit_request() {
    let err = ParseOutcome::Exit(2).into_run().expect_err("Exit is not a run");
    assert!(err.to_string().contains("exit request with code 2"), "{err}");
}

#[test]
fn test_parse_outcome_into_run_returns_the_plan() {
    let plan = ParseOutcome::Run(RunPlan {
        tasks: vec![],
        hash: "abc".to_string(),
        ottofile: None,
        jobs: 4,
        tui_mode: false,
        no_prefix: true,
    })
    .into_run()
    .expect("Run carries a plan");
    let (tasks, hash, ottofile, jobs, tui, no_prefix) = plan.into_parts();
    assert!(tasks.is_empty());
    assert_eq!(hash, "abc");
    assert!(ottofile.is_none());
    assert_eq!(jobs, 4);
    assert!(!tui);
    assert!(no_prefix);
}

/// A bare parser for the command-building tests. `task_to_command` and
/// `param_to_arg` became methods in Phase 6b (the Bind mode needs the
/// resolver and the ottofile directory), so these tests need an instance;
/// none of them load a config, so the default state is all they use.
fn test_parser() -> Parser {
    Parser::new(vec!["otto".to_string()]).expect("parser construction should not fail")
}

// =========================================================================
// ottofile_base_dir tests
// =========================================================================

#[test]
fn test_ottofile_base_dir_uses_parent() {
    let ottofile = PathBuf::from("/home/user/project/.otto.yml");
    let cwd = PathBuf::from("/some/other/place");
    assert_eq!(
        ottofile_base_dir(Some(&ottofile), &cwd),
        Path::new("/home/user/project")
    );
}

#[test]
fn test_ottofile_base_dir_ignores_invocation_cwd_when_ottofile_known() {
    // Regression: invoking otto from a subdirectory of a project must NOT
    // make the workspace root the subdirectory. The discovered ottofile's
    // parent wins over cwd.
    let ottofile = PathBuf::from("/home/user/project/.otto.yml");
    let cwd = PathBuf::from("/home/user/project/borg");
    assert_eq!(
        ottofile_base_dir(Some(&ottofile), &cwd),
        Path::new("/home/user/project")
    );
}

#[test]
fn test_ottofile_base_dir_filesystem_root_ottofile() {
    // PathBuf::from("/.otto.yml").parent() == Some("/"), a valid root.
    let ottofile = PathBuf::from("/.otto.yml");
    let cwd = PathBuf::from("/tmp");
    assert_eq!(ottofile_base_dir(Some(&ottofile), &cwd), Path::new("/"));
}

#[test]
fn test_ottofile_base_dir_none_falls_back_to_cwd() {
    let cwd = PathBuf::from("/some/cwd");
    assert_eq!(ottofile_base_dir(None, &cwd), Path::new("/some/cwd"));
}

#[test]
fn test_ottofile_base_dir_bare_filename_falls_back_to_cwd() {
    // A bare filename has parent == Some(""), which is not useful; fall back.
    // In practice the parser canonicalizes so this never happens, but
    // the helper must not produce a nonsense empty-path root.
    let ottofile = PathBuf::from(".otto.yml");
    let cwd = PathBuf::from("/some/cwd");
    assert_eq!(ottofile_base_dir(Some(&ottofile), &cwd), Path::new("/some/cwd"));
}

/// No task declares a value-taking option: the old partitioning behaviour.
fn no_value_options() -> ValueTakingOptions {
    ValueTakingOptions::new()
}

/// `task` takes a value for each token in `options`.
fn value_options(task: &str, options: &[&str]) -> ValueTakingOptions {
    let mut map = ValueTakingOptions::new();
    map.insert(
        task.to_string(),
        options.iter().map(|o| o.to_string()).collect::<HashSet<String>>(),
    );
    map
}

#[test]
fn test_indices() {
    let args = vec![
        "task1".to_string(),
        "arg2".to_string(),
        "task2".to_string(),
        "arg3".to_string(),
    ];
    let task_names = vec!["task1".to_string(), "task2".to_string()];
    let expected = vec![0, 2];
    assert_eq!(indices(&args, &task_names, &no_value_options()), expected);
}

#[test]
fn test_partitions() {
    let args = vec![
        "task1".to_string(),
        "arg2".to_string(),
        "task2".to_string(),
        "arg3".to_string(),
    ];
    let task_names = vec!["task1".to_string(), "task2".to_string()];
    let expected = vec![
        vec!["task1".to_string(), "arg2".to_string()],
        vec!["task2".to_string(), "arg3".to_string()],
    ];
    assert_eq!(partitions(&args, &task_names, &no_value_options()), expected);
}

#[test]
fn test_partitions_empty() {
    let args = vec!["arg1".to_string(), "arg2".to_string()];
    let task_names = vec!["task1".to_string(), "task2".to_string()];
    let expected: Vec<Vec<String>> = vec![];
    assert_eq!(partitions(&args, &task_names, &no_value_options()), expected);
}

#[test]
fn test_multiple_tasks_complex_args() {
    let args = vec![
        "build".to_string(),
        "--release".to_string(),
        "--target=x86_64-unknown-linux-gnu".to_string(),
        "test".to_string(),
        "--verbose".to_string(),
        "--filter=integration".to_string(),
        "deploy".to_string(),
        "--environment=staging".to_string(),
    ];

    let task_names = vec!["build".to_string(), "test".to_string(), "deploy".to_string()];
    let expected = vec![
        vec![
            "build".to_string(),
            "--release".to_string(),
            "--target=x86_64-unknown-linux-gnu".to_string(),
        ],
        vec![
            "test".to_string(),
            "--verbose".to_string(),
            "--filter=integration".to_string(),
        ],
        vec!["deploy".to_string(), "--environment=staging".to_string()],
    ];

    assert_eq!(partitions(&args, &task_names, &no_value_options()), expected);
}

#[test]
fn a_flag_value_that_spells_a_task_is_not_a_boundary() {
    // `otto build --msg test` used to split at `test`, leaving `--msg`
    // with no value and clap reporting a missing value the user supplied.
    let args = names(&["build", "--msg", "test"]);
    let task_names = names(&["build", "test"]);
    let expected = vec![names(&["build", "--msg", "test"])];
    assert_eq!(
        partitions(&args, &task_names, &value_options("build", &["--msg"])),
        expected
    );
}

#[test]
fn a_flag_value_does_not_hide_a_later_task() {
    let args = names(&["build", "--msg", "test", "lint"]);
    let task_names = names(&["build", "test", "lint"]);
    let expected = vec![names(&["build", "--msg", "test"]), names(&["lint"])];
    assert_eq!(
        partitions(&args, &task_names, &value_options("build", &["--msg"])),
        expected
    );
}

#[test]
fn a_boolean_flag_does_not_swallow_the_next_task() {
    let args = names(&["build", "--verbose", "test"]);
    let task_names = names(&["build", "test"]);
    let expected = vec![names(&["build", "--verbose"]), names(&["test"])];
    assert_eq!(
        partitions(&args, &task_names, &value_options("build", &["--msg"])),
        expected
    );
}

#[test]
fn a_double_dash_hands_the_rest_to_the_current_task() {
    // `otto build -- --msg=x` used to die with "unexpected argument".
    let args = names(&["build", "--", "--msg=x", "test"]);
    let task_names = names(&["build", "test"]);
    let expected = vec![names(&["build", "--msg=x", "test"])];
    assert_eq!(partitions(&args, &task_names, &no_value_options()), expected);
}

#[test]
fn duplicate_task_name_finds_the_repeat() {
    let parts = vec![names(&["build", "--msg=a"]), names(&["build", "--msg=b"])];
    assert_eq!(duplicate_task_name(&parts), Some("build"));
    assert!(duplicate_task_error("build").to_string().contains("more than once"));
}

#[test]
fn duplicate_task_name_accepts_distinct_tasks() {
    let parts = vec![names(&["build"]), names(&["test"])];
    assert_eq!(duplicate_task_name(&parts), None);
}

// =========================================================================
// ottofile value on the help path
// =========================================================================

#[test]
fn the_ottofile_flag_wins_over_the_env_and_the_default() {
    let args = names(&["otto", "-o", "sub/other.yml", "--help"]);
    assert_eq!(
        ottofile_value_from_args(&args, Some("env.yml".to_string())),
        OttofileSource::Explicit("sub/other.yml".to_string())
    );
}

#[test]
fn the_attached_ottofile_form_is_read_too() {
    let args = names(&["otto", "--ottofile=sub/other.yml", "--help"]);
    assert_eq!(
        ottofile_value_from_args(&args, None),
        OttofileSource::Explicit("sub/other.yml".to_string())
    );
}

#[test]
fn the_ottofile_env_is_used_when_no_flag_is_given() {
    let args = names(&["otto", "--help"]);
    assert_eq!(
        ottofile_value_from_args(&args, Some("env.yml".to_string())),
        OttofileSource::Explicit("env.yml".to_string())
    );
}

#[test]
fn the_ottofile_default_is_the_divine_variant_not_a_path() {
    let args = names(&["otto", "--help"]);
    assert_eq!(ottofile_value_from_args(&args, None), OttofileSource::Divine);
}

#[test]
fn an_ottofile_after_a_double_dash_is_not_ours() {
    let args = names(&["otto", "build", "--", "-o", "other.yml"]);
    assert_eq!(ottofile_value_from_args(&args, None), OttofileSource::Divine);
}

// =========================================================================
// --tui in task args, --Serial scanning, choices
// =========================================================================

#[test]
fn take_tui_flag_strips_the_flag_and_reports_it() {
    let (kept, found) = take_tui_flag(names(&["build", "--tui", "--msg=x"]), true);
    assert_eq!(kept, names(&["build", "--msg=x"]));
    assert!(found);
}

#[test]
fn take_tui_flag_strips_the_declared_short_too() {
    // `-t` is `--tui`'s declared short and global, so `otto build -t` has to
    // mean the same thing as `otto build --tui`; unstripped, it reached the
    // task's clap command as "unexpected argument '-t'".
    let (kept, found) = take_tui_flag(names(&["build", "-t"]), true);
    assert_eq!(kept, names(&["build"]));
    assert!(found);
}

#[test]
fn take_tui_flag_leaves_a_short_the_task_declares() {
    let (kept, found) = take_tui_flag(names(&["build", "-t", "unit"]), false);
    assert_eq!(kept, names(&["build", "-t", "unit"]));
    assert!(!found, "a task that declares -t owns it");
}

#[test]
fn take_tui_flag_leaves_other_args_alone() {
    let (kept, found) = take_tui_flag(names(&["build", "--msg=x"]), true);
    assert_eq!(kept, names(&["build", "--msg=x"]));
    assert!(!found);
}

/// A foreach subtask is addressed as `up:gamma`, which is not a key of the task
/// map, so the question "does a task in this arg list declare `-t`?" answered no
/// for every subtask and otto took the short away from the parent that declared
/// it: `otto up:gamma -t x` stripped `-t` as the TUI flag and `x` became a stray
/// positional clap refused.
#[test]
fn a_foreach_subtask_claims_its_parents_short_t() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");
    fs::write(
        &ottofile_path,
        "tasks:\n  up:\n    foreach:\n      items: [alpha, beta, gamma]\n    params:\n      -t|--target:\n        help: where to deploy\n    action: echo up\n",
    )
    .unwrap();

    let args = names(&[
        "otto",
        "--ottofile",
        ottofile_path.to_str().unwrap(),
        "up:gamma",
        "-t",
        "x",
    ]);
    let (tasks, _, _, _, tui_mode, _) = Parser::new(args)
        .unwrap()
        .parse()
        .expect("the subtask's own -t must not be stripped")
        .into_run()
        .unwrap()
        .into_parts();

    assert!(!tui_mode, "a task that declares -t owns it, subtask id included");
    let up = tasks
        .iter()
        .find(|t| t.name == "up:gamma")
        .expect("up:gamma must be in the run set");
    assert_eq!(up.values.get("target"), Some(&Value::Item("x".to_string())));
}

/// The same lookup gates `-h`: with the parent's `-h|--host` invisible behind
/// the subtask id, `otto up:gamma -h example.com` printed help and then failed
/// with `Task 'up:gamma' not found`.
#[test]
fn a_foreach_subtask_claims_its_parents_short_h() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");
    fs::write(
        &ottofile_path,
        "tasks:\n  up:\n    foreach:\n      items: [alpha, beta, gamma]\n    params:\n      -h|--host:\n        help: host to deploy to\n    action: echo up\n",
    )
    .unwrap();

    let args = names(&[
        "otto",
        "--ottofile",
        ottofile_path.to_str().unwrap(),
        "up:gamma",
        "-h",
        "example.com",
    ]);
    let (tasks, _, _, _, _, _) = Parser::new(args)
        .unwrap()
        .parse()
        .expect("help must not intercept a short the parent declares")
        .into_run()
        .unwrap()
        .into_parts();

    let up = tasks
        .iter()
        .find(|t| t.name == "up:gamma")
        .expect("up:gamma must be in the run set");
    assert_eq!(up.values.get("host"), Some(&Value::Item("example.com".to_string())));
}

#[test]
fn take_tui_flag_respects_a_double_dash() {
    let (kept, found) = take_tui_flag(names(&["build", "--", "--tui", "-t"]), true);
    assert_eq!(kept, names(&["build", "--", "--tui", "-t"]));
    assert!(!found, "after -- the flag belongs to the task");
}

// =========================================================================
// builtin/user task lists (Phase 5)
// =========================================================================

#[test]
fn a_mixed_task_list_is_rejected_naming_both_sides() {
    let err = reject_mixed_task_list(&names(&["build", "Clean"]))
        .expect_err("a builtin cannot be combined with an ordinary task")
        .to_string();
    assert!(err.contains("'build'"), "the error must name the task, got: {err}");
    assert!(err.contains("'Clean'"), "the error must name the builtin, got: {err}");
}

#[test]
fn a_mixed_task_list_names_every_offender() {
    let err = reject_mixed_task_list(&names(&["build", "Clean", "test", "Stats"]))
        .expect_err("a builtin cannot be combined with an ordinary task")
        .to_string();
    for name in ["build", "test", "Clean", "Stats"] {
        assert!(err.contains(&format!("'{name}'")), "missing '{name}' in: {err}");
    }
}

#[test]
fn an_all_user_task_list_is_accepted() {
    assert!(reject_mixed_task_list(&names(&["build", "test"])).is_ok());
}

#[test]
fn a_lone_builtin_is_accepted() {
    assert!(reject_mixed_task_list(&names(&["Clean"])).is_ok());
}

#[test]
fn an_empty_task_list_is_accepted() {
    assert!(reject_mixed_task_list(&[]).is_ok());
}

#[test]
fn contains_flag_ignores_a_value_that_spells_the_flag() {
    let options: HashSet<String> = ["--msg".to_string()].into_iter().collect();
    assert!(!contains_flag(
        &names(&["build", "--msg", "--Serial"]),
        "--Serial",
        Some(&options)
    ));
    assert!(contains_flag(
        &names(&["build", "--Serial"]),
        "--Serial",
        Some(&options)
    ));
}

#[test]
fn canonical_choice_matches_ignoring_case() {
    let choices = names(&["ascii", "unicode"]);
    assert_eq!(canonical_choice("ASCII", &choices), Some("ascii"));
    assert_eq!(canonical_choice("ascii", &choices), Some("ascii"));
    assert_eq!(canonical_choice("utf8", &choices), None);
}

#[test]
fn task_arg_error_names_the_equals_escape_hatch() {
    let err = task_arg_error(
        "a value is required".to_string(),
        clap::error::ErrorKind::InvalidValue,
        Some("test"),
    )
    .to_string();
    assert!(err.contains("--flag=test"), "{err}");
}

#[test]
fn task_arg_error_stays_quiet_when_nothing_was_split_off() {
    let err = task_arg_error(
        "a value is required".to_string(),
        clap::error::ErrorKind::InvalidValue,
        None,
    )
    .to_string();
    assert_eq!(err, "a value is required");
}

// New tests for flag functionality
use crate::cfg::param::{Nargs, ParamSpec, ParamType, Value};
use crate::cfg::task::TaskSpec;
use clap::Command;

fn create_test_param_spec(name: &str, param_type: ParamType, short: Option<char>, long: Option<&str>) -> ParamSpec {
    let default = match param_type {
        ParamType::FLG => Some("false".to_string()),
        _ => None,
    };

    ParamSpec {
        name: name.to_string(),
        short,
        long: long.map(|s| s.to_string()),
        param_type,
        metavar: None,
        default,
        choices_command: None,
        choices: vec![],
        nargs: Nargs::default(),
        help: Some(format!("Help for {name}")),
        value: Value::Empty,
        required: false,
    }
}

#[test]
fn test_param_to_arg_boolean_flag() {
    let param = create_test_param_spec("verbose", ParamType::FLG, Some('v'), Some("verbose"));
    let arg = test_parser().param_to_arg("test", &param, BuildMode::Bind).unwrap();

    // Test that the argument is configured correctly for boolean flags
    let cmd = Command::new("test").arg(arg.clone());
    let matches = cmd.try_get_matches_from(vec!["test", "--verbose"]).unwrap();

    assert!(matches.get_flag("verbose"));

    // Test without flag
    let cmd2 = Command::new("test").arg(arg);
    let matches = cmd2.try_get_matches_from(vec!["test"]).unwrap();
    assert!(!matches.get_flag("verbose"));
}

#[test]
fn test_param_to_arg_boolean_flag_short() {
    let param = create_test_param_spec("debug", ParamType::FLG, Some('d'), Some("debug"));
    let arg = test_parser().param_to_arg("test", &param, BuildMode::Bind).unwrap();

    // Test short form
    let cmd = Command::new("test").arg(arg.clone());
    let matches = cmd.try_get_matches_from(vec!["test", "-d"]).unwrap();
    assert!(matches.get_flag("debug"));

    // Test long form
    let cmd2 = Command::new("test").arg(arg);
    let matches = cmd2.try_get_matches_from(vec!["test", "--debug"]).unwrap();
    assert!(matches.get_flag("debug"));
}

#[test]
fn test_param_to_arg_string_argument() {
    let mut param = create_test_param_spec("env", ParamType::OPT, Some('e'), Some("env"));
    param.default = Some("development".to_string());

    let arg = test_parser().param_to_arg("test", &param, BuildMode::Bind).unwrap();

    // Test with explicit value
    let cmd = Command::new("test").arg(arg.clone());
    let matches = cmd.try_get_matches_from(vec!["test", "--env", "production"]).unwrap();
    assert_eq!(matches.get_one::<String>("env").unwrap(), "production");

    // Test with default value
    let cmd2 = Command::new("test").arg(arg);
    let matches = cmd2.try_get_matches_from(vec!["test"]).unwrap();
    assert_eq!(matches.get_one::<String>("env").unwrap(), "development");
}

/// `nargs` had zero readers outside `cfg/param.rs`; wiring it to
/// `num_args` lets a param collect more than one space-separated value
/// in a single occurrence, and downstream reads them all back via
/// `Value::List` rather than the first value alone.
#[test]
fn test_param_to_arg_nargs_one_or_more_collects_every_value() {
    let mut param = create_test_param_spec("files", ParamType::OPT, None, Some("files"));
    param.nargs = Nargs::OneOrMore;

    let arg = test_parser().param_to_arg("test", &param, BuildMode::Bind).unwrap();
    let cmd = Command::new("test").arg(arg);
    let matches = cmd
        .try_get_matches_from(vec!["test", "--files", "a.txt", "b.txt", "c.txt"])
        .unwrap();

    let values: Vec<&String> = matches.get_many::<String>("files").unwrap().collect();
    assert_eq!(values, vec!["a.txt", "b.txt", "c.txt"]);
}

#[test]
fn nargs_to_num_args_maps_every_variant() {
    assert_eq!(nargs_to_num_args(&Nargs::One), (1..=1).into());
    assert_eq!(nargs_to_num_args(&Nargs::Zero), (0..=0).into());
    assert_eq!(nargs_to_num_args(&Nargs::OneOrZero), (0..=1).into());
    assert_eq!(nargs_to_num_args(&Nargs::OneOrMore), (1..).into());
    assert_eq!(nargs_to_num_args(&Nargs::ZeroOrMore), (0..).into());
    assert_eq!(nargs_to_num_args(&Nargs::Range(2, 5)), (2..=5).into());
    // A bare `nargs: "3"` is `Range(3, 3)`: exactly three, which is what the
    // count means to clap and to argparse.
    assert_eq!(nargs_to_num_args(&Nargs::Range(3, 3)), (3..=3).into());
}

#[test]
fn test_param_to_arg_with_choices() {
    let mut param = create_test_param_spec("format", ParamType::OPT, Some('f'), Some("format"));
    param.choices = vec!["json".to_string(), "yaml".to_string(), "xml".to_string()];
    param.default = Some("json".to_string());

    let arg = test_parser().param_to_arg("test", &param, BuildMode::Bind).unwrap();

    // Test valid choice
    let cmd = Command::new("test").arg(arg.clone());
    let matches = cmd.try_get_matches_from(vec!["test", "--format", "yaml"]).unwrap();
    assert_eq!(matches.get_one::<String>("format").unwrap(), "yaml");

    // Test invalid choice should fail
    let cmd2 = Command::new("test").arg(arg);
    let result = cmd2.try_get_matches_from(vec!["test", "--format", "invalid"]);
    assert!(result.is_err());
}

#[test]
fn test_param_to_arg_positional() {
    let mut param = create_test_param_spec("filename", ParamType::POS, None, None);
    param.metavar = Some("FILE".to_string());

    let arg = test_parser().param_to_arg("test", &param, BuildMode::Bind).unwrap();
    let cmd = Command::new("test").arg(arg);

    let matches = cmd.try_get_matches_from(vec!["test", "input.txt"]).unwrap();
    assert_eq!(matches.get_one::<String>("filename").unwrap(), "input.txt");
}

#[test]
fn test_task_to_command_mixed_parameters() {
    let mut task_spec = TaskSpec {
        name: "build".to_string(),
        help: Some("Build the project".to_string()),
        ..Default::default()
    };

    let verbose_param = create_test_param_spec("verbose", ParamType::FLG, Some('v'), Some("verbose"));
    task_spec.params.insert("verbose".to_string(), verbose_param);

    let mut env_param = create_test_param_spec("env", ParamType::OPT, Some('e'), Some("env"));
    env_param.default = Some("development".to_string());
    env_param.choices = vec![
        "development".to_string(),
        "staging".to_string(),
        "production".to_string(),
    ];
    task_spec.params.insert("env".to_string(), env_param);

    let filename_param = create_test_param_spec("filename", ParamType::POS, None, None);
    task_spec.params.insert("filename".to_string(), filename_param);

    let cmd = test_parser().task_to_command(&task_spec, BuildMode::Bind).unwrap();

    // Test with all parameters
    let matches = cmd
        .try_get_matches_from(vec!["build", "--verbose", "--env", "production", "input.txt"])
        .unwrap();

    assert!(matches.get_flag("verbose"));
    assert_eq!(matches.get_one::<String>("env").unwrap(), "production");
    assert_eq!(matches.get_one::<String>("filename").unwrap(), "input.txt");
}

#[test]
fn test_task_to_command_boolean_flags_only() {
    let mut task_spec = TaskSpec {
        name: "test".to_string(),
        ..Default::default()
    };

    let verbose_param = create_test_param_spec("verbose", ParamType::FLG, Some('v'), Some("verbose"));
    task_spec.params.insert("verbose".to_string(), verbose_param);

    let coverage_param = create_test_param_spec("coverage", ParamType::FLG, None, Some("coverage"));
    task_spec.params.insert("coverage".to_string(), coverage_param);

    let watch_param = create_test_param_spec("watch", ParamType::FLG, Some('w'), Some("watch"));
    task_spec.params.insert("watch".to_string(), watch_param);

    // Test with all flags
    let cmd = test_parser().task_to_command(&task_spec, BuildMode::Bind).unwrap();
    let matches = cmd
        .try_get_matches_from(vec!["test", "-v", "--coverage", "-w"])
        .unwrap();
    assert!(matches.get_flag("verbose"));
    assert!(matches.get_flag("coverage"));
    assert!(matches.get_flag("watch"));

    // Test with no flags
    let cmd2 = test_parser().task_to_command(&task_spec, BuildMode::Bind).unwrap();
    let matches = cmd2.try_get_matches_from(vec!["test"]).unwrap();
    assert!(!matches.get_flag("verbose"));
    assert!(!matches.get_flag("coverage"));
    assert!(!matches.get_flag("watch"));
}

#[test]
fn test_default_jobs_value() {
    // Test that DEFAULT_JOBS equals default_jobs()
    let expected = default_jobs().to_string();
    assert_eq!(DEFAULT_JOBS.as_str(), expected);
}

#[test]
fn test_jobs_parameter_parsing() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");
    fs::write(&ottofile_path, "tasks:\n  test:\n    action: echo test\n").unwrap();

    // Test with explicit jobs value
    let args = vec![
        "otto".to_string(),
        "-j".to_string(),
        "4".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "test".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse();
    assert!(result.is_ok());
    let (_, _, _, jobs, _, _) = result.unwrap().into_run().unwrap().into_parts();
    assert_eq!(jobs, 4);
}

#[test]
fn test_jobs_parameter_default() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");
    fs::write(&ottofile_path, "tasks:\n  test:\n    action: echo test\n").unwrap();

    // Test without explicit jobs value (should default to default_jobs())
    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "test".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse();
    assert!(result.is_ok());
    let (_, _, _, jobs, _, _) = result.unwrap().into_run().unwrap().into_parts();
    assert_eq!(jobs, default_jobs());
}

/// Inverted deliberately: this test used to assert that `-j invalid`
/// silently fell back to `default_jobs()`. A concurrency limit the operator
/// typed and otto ignored is exactly the class of silent success this phase
/// closes, so the value parser now rejects it.
#[test]
fn test_jobs_parameter_invalid_is_rejected() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");
    fs::write(&ottofile_path, "tasks:\n  test:\n    action: echo test\n").unwrap();

    let args = vec![
        "otto".to_string(),
        "-j".to_string(),
        "invalid".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "test".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let err = parser
        .parse()
        .expect_err("-j invalid must be rejected, not silently defaulted");
    assert!(
        err.to_string().contains("invalid value 'invalid'"),
        "error should name the rejected value, got: {err}"
    );
}

/// `-j 0` used to be accepted and then hot-spin the launch loop at ~100% CPU
/// forever, because `while active_tasks.len() < 0` never admits a task.
#[test]
fn test_jobs_zero_is_rejected_at_parse() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");
    fs::write(&ottofile_path, "tasks:\n  test:\n    action: echo test\n").unwrap();

    let args = vec![
        "otto".to_string(),
        "-j".to_string(),
        "0".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "test".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let err = parser.parse().expect_err("-j 0 must be rejected at parse");
    let msg = err.to_string();
    assert!(
        msg.contains("invalid value '0'") && msg.contains("not in 1.."),
        "error should say 0 is out of range, got: {msg}"
    );
}

/// `otto.jobs` used to parse and do nothing (design doc Phase 10, the
/// "inert otto: keys" bullet). It is now the default concurrency when
/// `-j/--jobs` is not given explicitly.
#[test]
fn test_otto_jobs_config_sets_default_when_flag_omitted() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");
    fs::write(
        &ottofile_path,
        "otto:\n  jobs: 3\ntasks:\n  test:\n    action: echo test\n",
    )
    .unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "test".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse();
    assert!(result.is_ok());
    let (_, _, _, jobs, _, _) = result.unwrap().into_run().unwrap().into_parts();
    assert_eq!(jobs, 3);
}

/// An explicit `-j` on the command line wins over `otto.jobs` even when the
/// two disagree.
#[test]
fn test_explicit_jobs_flag_overrides_otto_jobs_config() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");
    fs::write(
        &ottofile_path,
        "otto:\n  jobs: 3\ntasks:\n  test:\n    action: echo test\n",
    )
    .unwrap();

    let args = vec![
        "otto".to_string(),
        "-j".to_string(),
        "7".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "test".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse();
    assert!(result.is_ok());
    let (_, _, _, jobs, _, _) = result.unwrap().into_run().unwrap().into_parts();
    assert_eq!(jobs, 7);
}

// Tests for collect_transitive_deps and after semantic
#[test]
fn test_collect_transitive_deps_basic() {
    let mut task_deps = HashMap::new();
    task_deps.insert("a".to_string(), vec![]);
    task_deps.insert("b".to_string(), vec![TaskEdge::success("a")]);
    task_deps.insert("c".to_string(), vec![TaskEdge::success("b")]);

    let task_specs = TaskSpecs::new();
    let mut collected = HashSet::new();

    Parser::collect_transitive_deps("c", &task_deps, &task_specs, &mut collected).unwrap();

    assert!(collected.contains("a"));
    assert!(collected.contains("b"));
    assert!(collected.contains("c"));
    assert_eq!(collected.len(), 3);
}

#[test]
fn test_collect_transitive_deps_with_after() {
    // Test that 'after' tasks are automatically included
    let mut task_deps = HashMap::new();
    task_deps.insert("cov".to_string(), vec![]);
    task_deps.insert("cov-report".to_string(), vec![TaskEdge::success("cov")]);

    let mut task_specs = TaskSpecs::new();
    let cov_spec = TaskSpec {
        name: "cov".to_string(),
        after: vec![crate::cfg::edge::EdgeSpec::sugar("cov-report")],
        ..Default::default()
    };
    task_specs.insert("cov".to_string(), cov_spec);

    let cov_report_spec = TaskSpec {
        name: "cov-report".to_string(),
        ..Default::default()
    };
    task_specs.insert("cov-report".to_string(), cov_report_spec);

    let mut collected = HashSet::new();

    // Running "cov" should also include "cov-report" due to after
    Parser::collect_transitive_deps("cov", &task_deps, &task_specs, &mut collected).unwrap();

    assert!(collected.contains("cov"), "cov should be included");
    assert!(
        collected.contains("cov-report"),
        "cov-report should be auto-included via after"
    );
    assert_eq!(collected.len(), 2);
}

#[test]
fn test_collect_transitive_deps_after_chain() {
    // Test chained after: a -> after: [b] -> after: [c]
    let task_deps = HashMap::new();

    let mut task_specs = TaskSpecs::new();

    let a_spec = TaskSpec {
        name: "a".to_string(),
        after: vec![crate::cfg::edge::EdgeSpec::sugar("b")],
        ..Default::default()
    };
    task_specs.insert("a".to_string(), a_spec);

    let b_spec = TaskSpec {
        name: "b".to_string(),
        after: vec![crate::cfg::edge::EdgeSpec::sugar("c")],
        ..Default::default()
    };
    task_specs.insert("b".to_string(), b_spec);

    let c_spec = TaskSpec {
        name: "c".to_string(),
        ..Default::default()
    };
    task_specs.insert("c".to_string(), c_spec);

    let mut collected = HashSet::new();

    // Running "a" should include a, b, and c (through the after chain)
    Parser::collect_transitive_deps("a", &task_deps, &task_specs, &mut collected).unwrap();

    assert!(collected.contains("a"));
    assert!(collected.contains("b"));
    assert!(collected.contains("c"));
    assert_eq!(collected.len(), 3);
}

#[test]
fn test_collect_transitive_deps_after_with_dependencies() {
    // Test: a has after: [b], and b has before: [dep]
    // Running a should include: a, b, and dep
    let mut task_deps = HashMap::new();
    task_deps.insert("a".to_string(), vec![]);
    task_deps.insert("b".to_string(), vec![TaskEdge::success("dep")]);
    task_deps.insert("dep".to_string(), vec![]);

    let mut task_specs = TaskSpecs::new();

    let a_spec = TaskSpec {
        name: "a".to_string(),
        after: vec![crate::cfg::edge::EdgeSpec::sugar("b")],
        ..Default::default()
    };
    task_specs.insert("a".to_string(), a_spec);

    let b_spec = TaskSpec {
        name: "b".to_string(),
        ..Default::default()
    };
    task_specs.insert("b".to_string(), b_spec);

    let dep_spec = TaskSpec {
        name: "dep".to_string(),
        ..Default::default()
    };
    task_specs.insert("dep".to_string(), dep_spec);

    let mut collected = HashSet::new();

    Parser::collect_transitive_deps("a", &task_deps, &task_specs, &mut collected).unwrap();

    assert!(collected.contains("a"));
    assert!(collected.contains("b"));
    assert!(collected.contains("dep"), "dep should be included as b's dependency");
    assert_eq!(collected.len(), 3);
}

#[test]
fn test_collect_transitive_deps_no_duplicates() {
    // Test that circular references via after don't cause infinite loops
    let mut task_deps = HashMap::new();
    task_deps.insert("a".to_string(), vec![]);
    task_deps.insert("b".to_string(), vec![TaskEdge::success("a")]);

    let mut task_specs = TaskSpecs::new();

    let a_spec = TaskSpec {
        name: "a".to_string(),
        after: vec![crate::cfg::edge::EdgeSpec::sugar("b")],
        ..Default::default()
    };
    task_specs.insert("a".to_string(), a_spec);

    let b_spec = TaskSpec {
        name: "b".to_string(),
        after: vec![crate::cfg::edge::EdgeSpec::sugar("a")], // Circular after reference
        ..Default::default()
    };
    task_specs.insert("b".to_string(), b_spec);

    let mut collected = HashSet::new();

    // Should not panic or infinite loop
    Parser::collect_transitive_deps("a", &task_deps, &task_specs, &mut collected).unwrap();

    assert!(collected.contains("a"));
    assert!(collected.contains("b"));
    assert_eq!(collected.len(), 2);
}

// Tests for foreach parallel: false feature

// ------------------------------------------------------------------
// foreach: command: lazy-resolution seams (Phase 6)
// ------------------------------------------------------------------

#[test]
fn test_args_mention_task_matches_parent_and_subtask_tokens() {
    let args = vec!["up:gamma".to_string(), "--flag".to_string()];
    assert!(Parser::args_mention_task(&args, "up"));
    assert!(!Parser::args_mention_task(&args, "upgrade"));
    assert!(!Parser::args_mention_task(&args, "build"));

    let args = vec!["up".to_string()];
    assert!(Parser::args_mention_task(&args, "up"));
}

#[test]
fn test_parent_task_name_strips_the_subtask_suffix() {
    assert_eq!(Parser::parent_task_name("up:gamma"), "up");
    assert_eq!(Parser::parent_task_name("up"), "up");
}

#[test]
fn test_reachable_task_names_covers_both_edge_directions() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");
    // deps <- build (build requires deps); notify runs after build;
    // lonely is connected to nothing.
    let config = r#"
tasks:
  deps:
    bash: echo deps
  build:
    before: [deps]
    bash: echo build
  notify:
    after: [build]
    bash: echo notify
  lonely:
    bash: echo lonely
"#;
    fs::write(&ottofile_path, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "build".to_string(),
    ];
    let mut parser = Parser::new(args).unwrap();
    parser.parse().unwrap();

    let reachable = parser.reachable_task_names(&["build".to_string()]);
    assert!(reachable.contains("build"));
    assert!(reachable.contains("deps"), "upstream `before:` target is reachable");
    assert!(
        reachable.contains("notify"),
        "a task whose `after:` names build is pulled in by build"
    );
    assert!(!reachable.contains("lonely"), "an unrelated task is not reachable");

    // a subtask-shaped root collapses to its parent
    let reachable = parser.reachable_task_names(&["build:one".to_string()]);
    assert!(reachable.contains("build"));
}

#[test]
fn test_help_renders_dynamic_for_a_command_sourced_foreach() {
    let mut task_spec = TaskSpec {
        name: "up".to_string(),
        help: Some("Bring up each service".to_string()),
        action: "echo ${svc}".to_string(),
        ..Default::default()
    };
    task_spec.foreach = Some(ForeachSpec {
        // If help ever resolved this, the sentinel file would appear.
        command: Some("printf 'alpha\n'".to_string()),
        var_name: "svc".to_string(),
        ..Default::default()
    });

    let rendered = test_parser()
        .task_to_command_for_help(&task_spec)
        .render_long_help()
        .to_string();

    assert!(rendered.contains("[dynamic]"), "{rendered}");
    assert!(!rendered.contains("items]"), "{rendered}");
}

#[test]
fn test_help_still_renders_item_counts_for_static_foreach() {
    let mut task_spec = TaskSpec {
        name: "up".to_string(),
        help: Some("Bring up each service".to_string()),
        action: "echo ${svc}".to_string(),
        ..Default::default()
    };
    task_spec.foreach = Some(ForeachSpec {
        items: vec!["alpha".to_string(), "beta".to_string()],
        var_name: "svc".to_string(),
        ..Default::default()
    });

    let rendered = test_parser()
        .task_to_command_for_help(&task_spec)
        .render_long_help()
        .to_string();

    assert!(rendered.contains("[2 items]"), "{rendered}");
}

/// Rewritten in Phase 4 of docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md:
/// serial ordering used to be sibling `before:` edges, which made "runs after" mean
/// "requires". It is now group membership plus an order index.
#[test]
fn test_foreach_subtasks_grouped_when_parallel_false() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");

    // Create an ottofile with parallel: false
    let config = r#"
tasks:
  install:
    foreach:
      items: [a, b, c]
      as: pkg
      parallel: false
    bash: echo ${pkg}
"#;
    fs::write(&ottofile_path, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "install".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse().unwrap();
    let (tasks, _, _, _, _, _) = result.into_run().unwrap().into_parts();

    // Find the subtasks and verify they are chained
    let subtask_a = tasks.iter().find(|t| t.name == "install:a");
    let subtask_b = tasks.iter().find(|t| t.name == "install:b");
    let subtask_c = tasks.iter().find(|t| t.name == "install:c");

    assert!(subtask_a.is_some(), "subtask install:a should exist");
    assert!(subtask_b.is_some(), "subtask install:b should exist");
    assert!(subtask_c.is_some(), "subtask install:c should exist");

    // With parallel: false, subtasks join one serial group in declared order and
    // carry NO sibling edges - ordering must not pull siblings into the run set.
    let a = subtask_a.unwrap();
    let b = subtask_b.unwrap();
    let c = subtask_c.unwrap();

    for (task, index) in [(a, 0), (b, 1), (c, 2)] {
        assert_eq!(
            task.serial_group.as_deref(),
            Some("install"),
            "{} should be in serial group 'install'",
            task.name
        );
        assert_eq!(task.serial_index, index, "{} order index", task.name);
        assert!(
            !task.task_deps.iter().any(|d| d.task.starts_with("install:")),
            "{} should carry no sibling edge, got: {:?}",
            task.name,
            task.task_deps
        );
    }
}

/// `foreach.jobs` reaches the scheduler as a resolved permit count on the
/// group's ITEMS and on nothing else.
///
/// `all` is the count the items expanded to, which is the only place that
/// number exists: the scheduler sees subtasks, never the `foreach:` block. The
/// virtual parent is deliberately left `None` - it is queued only once its
/// items are terminal, so it never runs beside them and an exemption there
/// would be a carve-out for a task that cannot use one (design doc
/// `2026-09-01-cancellation-reaping-and-foreach-concurrency.md`, Phase 3).
#[test]
fn foreach_jobs_is_stamped_on_every_item_and_never_on_the_virtual_parent() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");
    fs::write(
        &ottofile_path,
        "tasks:\n  \
         tail:\n    \
         foreach:\n      \
         items: [alpha, beta, gamma]\n      \
         parallel: true\n      \
         jobs: all\n    \
         action: echo ${item}\n  \
         capped:\n    \
         foreach:\n      \
         items: [one, two]\n      \
         parallel: true\n      \
         jobs: 1\n    \
         action: echo ${item}\n",
    )
    .unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "tail".to_string(),
        "capped".to_string(),
    ];
    let mut parser = Parser::new(args).unwrap();
    let (tasks, _, _, _, _, _) = parser.parse().unwrap().into_run().unwrap().into_parts();

    let jobs_for = |name: &str| {
        tasks
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("task {name} missing from the run set"))
            .foreach_jobs
            .map(std::num::NonZeroUsize::get)
    };

    // `all` over three items is three permits, on each of the three items.
    for item in ["tail:alpha", "tail:beta", "tail:gamma"] {
        assert_eq!(jobs_for(item), Some(3), "{item} carries one permit per item");
    }
    assert_eq!(jobs_for("tail"), None, "the virtual parent is never exempt");

    // A fixed count is carried verbatim, and does not leak to the other group.
    assert_eq!(jobs_for("capped:one"), Some(1));
    assert_eq!(jobs_for("capped:two"), Some(1));
    assert_eq!(jobs_for("capped"), None);
}
