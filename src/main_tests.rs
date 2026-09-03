#![cfg(test)]

use super::*;
use serial_test::serial;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_rotate_log_no_file() {
    let temp_dir = TempDir::new().unwrap();
    let log_path = temp_dir.path().join("otto.log");
    // Should not panic when file doesn't exist
    rotate_log_if_needed(&log_path);
    assert!(!log_path.exists());
}

#[test]
fn test_rotate_log_small_file() {
    let temp_dir = TempDir::new().unwrap();
    let log_path = temp_dir.path().join("otto.log");
    fs::write(&log_path, "small content").unwrap();
    rotate_log_if_needed(&log_path);
    // File should still exist (not rotated)
    assert!(log_path.exists());
    assert!(!temp_dir.path().join("otto.log.1").exists());
}

#[test]
fn test_rotate_log_oversized_file() {
    let temp_dir = TempDir::new().unwrap();
    let log_path = temp_dir.path().join("otto.log");
    // Create a file larger than 10MB
    let content = vec![b'x'; 11 * 1024 * 1024];
    fs::write(&log_path, &content).unwrap();

    rotate_log_if_needed(&log_path);

    // Original should be gone, backup should exist
    assert!(!log_path.exists());
    let backup = temp_dir.path().join("otto.log.1");
    assert!(backup.exists());
    assert_eq!(fs::metadata(&backup).unwrap().len(), content.len() as u64);
}

#[test]
fn test_rotate_log_overwrites_existing_backup() {
    let temp_dir = TempDir::new().unwrap();
    let log_path = temp_dir.path().join("otto.log");
    let backup_path = temp_dir.path().join("otto.log.1");

    // Create old backup
    fs::write(&backup_path, "old backup").unwrap();
    // Create oversized log
    let content = vec![b'y'; 11 * 1024 * 1024];
    fs::write(&log_path, &content).unwrap();

    rotate_log_if_needed(&log_path);

    // Backup should be overwritten with new content
    assert!(!log_path.exists());
    assert!(backup_path.exists());
    assert_eq!(fs::metadata(&backup_path).unwrap().len(), content.len() as u64);
}

#[test]
fn test_apply_cwd_flag_no_flag() {
    let args = vec!["otto".into(), "ci".into()];
    let result = apply_cwd_flag(args.clone()).unwrap();
    assert_eq!(result, args);
}

#[test]
#[serial]
fn test_apply_cwd_flag_short() {
    let original_cwd = env::current_dir().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let dir = temp_dir.path().to_string_lossy().to_string();

    let args = vec!["otto".into(), "-C".into(), dir.clone(), "ci".into()];
    let result = apply_cwd_flag(args).unwrap();

    assert_eq!(result, vec!["otto", "ci"]);
    let new_cwd = env::current_dir().unwrap();
    assert_eq!(new_cwd.canonicalize().unwrap(), temp_dir.path().canonicalize().unwrap());

    env::set_current_dir(original_cwd).unwrap();
}

#[test]
#[serial]
fn test_apply_cwd_flag_long() {
    let original_cwd = env::current_dir().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let dir = temp_dir.path().to_string_lossy().to_string();

    let args = vec!["otto".into(), "--cwd".into(), dir.clone(), "ci".into()];
    let result = apply_cwd_flag(args).unwrap();

    assert_eq!(result, vec!["otto", "ci"]);
    env::set_current_dir(original_cwd).unwrap();
}

#[test]
#[serial]
fn test_apply_cwd_flag_equals_form() {
    let original_cwd = env::current_dir().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let dir = temp_dir.path().to_string_lossy().to_string();

    let args = vec!["otto".into(), format!("--cwd={}", dir), "ci".into()];
    let result = apply_cwd_flag(args).unwrap();

    assert_eq!(result, vec!["otto", "ci"]);
    env::set_current_dir(original_cwd).unwrap();
}

#[test]
#[serial]
fn test_apply_cwd_flag_short_equals_form() {
    // Attached `-C=DIR` form: clap silently swallows this today because
    // the parser-side `-C` Arg doesn't accept an attached `=` value for a
    // short flag. apply_cwd_flag must strip it before clap ever sees it.
    let original_cwd = env::current_dir().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let dir = temp_dir.path().to_string_lossy().to_string();

    let args = vec!["otto".into(), format!("-C={}", dir), "ci".into()];
    let result = apply_cwd_flag(args).unwrap();

    assert_eq!(result, vec!["otto", "ci"]);
    let new_cwd = env::current_dir().unwrap();
    assert_eq!(new_cwd.canonicalize().unwrap(), temp_dir.path().canonicalize().unwrap());

    env::set_current_dir(original_cwd).unwrap();
}

#[test]
fn test_apply_cwd_flag_nonexistent_dir() {
    let args = vec!["otto".into(), "-C".into(), "/nonexistent/path/xyz".into()];
    let result = apply_cwd_flag(args);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("does not exist"));
}

#[test]
fn test_apply_cwd_flag_missing_value() {
    let args = vec!["otto".into(), "-C".into()];
    let result = apply_cwd_flag(args);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("requires a directory argument"));
}

// =====================================================================
// --log-level
// =====================================================================

#[test]
fn log_level_absent_leaves_the_args_alone() {
    let args = vec!["otto".to_string(), "ci".to_string()];
    let (kept, level) = apply_log_level_flag(args.clone()).unwrap();
    assert_eq!(kept, args);
    assert_eq!(level, None);
}

#[test]
fn log_level_is_stripped_in_both_forms() {
    let (kept, level) =
        apply_log_level_flag(vec!["otto".into(), "--log-level".into(), "debug".into(), "ci".into()]).unwrap();
    assert_eq!(kept, vec!["otto", "ci"]);
    assert_eq!(level.as_deref(), Some("debug"));

    let (kept, level) = apply_log_level_flag(vec!["otto".into(), "--log-level=trace".into(), "ci".into()]).unwrap();
    assert_eq!(kept, vec!["otto", "ci"]);
    assert_eq!(level.as_deref(), Some("trace"));
}

#[test]
fn log_level_is_case_insensitive() {
    let (_, level) = apply_log_level_flag(vec!["otto".into(), "--log-level=DEBUG".into()]).unwrap();
    assert_eq!(level.as_deref(), Some("debug"));
}

#[test]
fn an_unknown_log_level_is_an_error() {
    let err = apply_log_level_flag(vec!["otto".into(), "--log-level=bogus".into()])
        .expect_err("bogus is not a level")
        .to_string();
    assert!(err.contains("invalid --log-level 'bogus'"), "{err}");
    assert!(err.contains("debug"), "the error lists the accepted values: {err}");
}

#[test]
fn a_valueless_log_level_is_an_error() {
    let err = apply_log_level_flag(vec!["otto".into(), "--log-level".into()])
        .expect_err("the flag needs a value")
        .to_string();
    assert!(err.contains("requires a value"), "{err}");
}

#[test]
fn log_level_after_a_double_dash_belongs_to_the_task() {
    let args = vec!["otto".into(), "build".into(), "--".into(), "--log-level=debug".into()];
    let (kept, level) = apply_log_level_flag(args.clone()).unwrap();
    assert_eq!(kept, args);
    assert_eq!(level, None);
}

// =====================================================================
// builtin routing past global flags
// =====================================================================

#[test]
fn the_first_command_is_found_behind_global_flags() {
    let args: Vec<String> = ["otto", "-j", "2", "--tui", "Convert"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(first_command_index(&args), Some(4));
}

#[test]
fn a_bare_command_is_still_found() {
    let args: Vec<String> = ["otto", "Clean"].iter().map(|s| s.to_string()).collect();
    assert_eq!(first_command_index(&args), Some(1));
}

#[test]
fn an_attached_value_flag_consumes_only_itself() {
    let args: Vec<String> = ["otto", "--ottofile=x.yml", "Stats"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(first_command_index(&args), Some(2));
}

#[test]
fn a_double_dash_ends_the_search() {
    let args: Vec<String> = ["otto", "--", "Clean"].iter().map(|s| s.to_string()).collect();
    assert_eq!(first_command_index(&args), None);
}

#[test]
fn flags_alone_name_no_command() {
    let args: Vec<String> = ["otto", "-j", "2"].iter().map(|s| s.to_string()).collect();
    assert_eq!(first_command_index(&args), None);
}

#[test]
fn test_apply_cwd_flag_stops_at_double_dash() {
    let args = vec!["otto".into(), "ci".into(), "--".into(), "-C".into(), "/tmp".into()];
    let result = apply_cwd_flag(args.clone()).unwrap();
    // -C after -- should not be consumed
    assert_eq!(result, args);
}

// =========================================================================
// Early builtin routing (Phase 5)
// =========================================================================

#[test]
fn every_builtin_but_graph_is_early_routed() {
    // The two lists are deliberately different by exactly one entry. `Graph`
    // needs the parsed ottofile's task specs, so it has no early route; every
    // other builtin must have one, or `otto NAME --flag` falls through to the
    // task parser and dies on a flag the builtin's own clap parser declares.
    for builtin in otto::app::Builtin::all() {
        let route = EarlyCommand::from_name(builtin.task_name());
        if builtin == otto::app::Builtin::Graph {
            assert!(
                route.is_none(),
                "Graph must stay on the task route: it needs the ottofile's task specs"
            );
        } else {
            assert!(
                route.is_some(),
                "builtin '{}' has no early route in handle_subcommand",
                builtin.task_name()
            );
        }
    }
}

#[test]
fn an_ordinary_task_name_has_no_early_route() {
    assert_eq!(EarlyCommand::from_name("build"), None);
    // Case matters: the builtins are capitalized precisely so a lowercase task
    // name of the same word is the user's.
    assert_eq!(EarlyCommand::from_name("clean"), None);
}
