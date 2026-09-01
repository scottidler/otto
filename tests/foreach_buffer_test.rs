//! Integration tests for `foreach.buffer` (design doc
//! `docs/design/2026-08-31-buffered-foreach-computed-envs-required-params.md`,
//! Phase 3: the display-order map and the schema key only. No emission-order
//! behavior lands until Phase 4; these tests pin the load-time validation and
//! the additive map, not any change to what prints when.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use tempfile::TempDir;

use otto::Parser;

fn write_ottofile(dir: &Path, contents: &str) -> PathBuf {
    let path = dir.join("otto.yml");
    fs::write(&path, contents).unwrap();
    path
}

fn otto(ottofile: &Path, args: &[&str]) -> Output {
    let home = ottofile.parent().expect("ottofile must live in a directory");
    common::otto_cmd(home)
        .arg("-o")
        .arg(ottofile)
        .args(args)
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// Success criterion (b): a `parallel: true` expansion's display-order map
/// entry lists subtask names in declared item order, matching each subtask's
/// own `OTTO_FOREACH_INDEX`. Read straight off `cli::parser::Task` (no
/// scheduler involved): the map is inert this phase, so there is nothing to
/// execute yet.
#[test]
fn test_display_order_map_matches_declared_item_order_and_foreach_index() -> eyre::Result<()> {
    let temp_dir = TempDir::new()?;
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  say:
    foreach:
      items: [alpha, beta, gamma]
      as: item
      parallel: true
    bash: echo ${item}
"#,
    );

    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        ottofile.to_string_lossy().to_string(),
        "say".to_string(),
    ];
    let mut parser = Parser::new(args)?;
    let (parser_tasks, _, _, _, _, _) = parser.parse()?.into_run()?.into_parts();

    let parent = parser_tasks
        .iter()
        .find(|t| t.name == "say")
        .expect("virtual parent 'say' must be in the run set");
    let expected: Vec<String> = ["alpha", "beta", "gamma"]
        .iter()
        .map(|item| format!("say:{item}"))
        .collect();
    assert_eq!(
        parent.foreach_display_order,
        Some(expected.clone()),
        "the display-order map must list subtask names in declared item order"
    );

    // Cross-check against OTTO_FOREACH_INDEX: the map and the env var must
    // never drift, since Phase 4's replay cursor trusts both to agree.
    for (index, subtask_name) in expected.iter().enumerate() {
        let subtask = parser_tasks
            .iter()
            .find(|t| &t.name == subtask_name)
            .unwrap_or_else(|| panic!("subtask '{subtask_name}' must be in the run set"));
        assert_eq!(
            subtask.envs.get("OTTO_FOREACH_INDEX"),
            Some(&index.to_string()),
            "subtask '{subtask_name}' OTTO_FOREACH_INDEX must match its position in the display-order map"
        );
    }
    Ok(())
}

/// A non-foreach task, and a foreach parent whose task the run set never
/// reaches, both carry no display-order entry: the map is additive, not a
/// default-populated field.
#[test]
fn test_display_order_is_none_for_a_non_foreach_task() -> eyre::Result<()> {
    let temp_dir = TempDir::new()?;
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  plain:
    bash: echo hi
"#,
    );

    let args = vec![
        "otto".to_string(),
        "-o".to_string(),
        ottofile.to_string_lossy().to_string(),
        "plain".to_string(),
    ];
    let mut parser = Parser::new(args)?;
    let (parser_tasks, _, _, _, _, _) = parser.parse()?.into_run()?.into_parts();

    let plain = parser_tasks
        .iter()
        .find(|t| t.name == "plain")
        .expect("task must exist");
    assert_eq!(plain.foreach_display_order, None);
    Ok(())
}

/// Success criterion (c), first half: `buffer: true` combined with `tty: true`
/// on the same task is a load error naming both keys.
#[test]
fn test_buffer_with_tty_fails_to_load_naming_both_keys() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  status:
    tty: true
    foreach:
      items: [alpha, beta]
      buffer: true
    bash: echo ${item}
"#,
    );

    let output = otto(&ottofile, &["status"]);
    assert!(!output.status.success(), "buffer + tty must fail to load");
    let err = stderr(&output);
    assert!(err.contains("buffer"), "error must name 'buffer': {err}");
    assert!(err.contains("tty"), "error must name 'tty': {err}");
}

/// Success criterion (c), second half: `tty: true` on a foreach task WITHOUT
/// `buffer` keeps working exactly as verified on main (unprefixed contiguous
/// blocks) - Phase 3 must not regress it.
#[test]
fn test_tty_without_buffer_on_a_foreach_task_still_runs() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  status:
    tty: true
    foreach:
      items: [alpha, beta]
      parallel: true
    bash: echo ${item}
"#,
    );

    let output = otto(&ottofile, &["status"]);
    assert!(
        output.status.success(),
        "tty + foreach without buffer must still run: {}",
        stderr(&output)
    );
}
