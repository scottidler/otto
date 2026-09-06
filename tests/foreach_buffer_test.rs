//! Integration tests for `foreach.buffer` (design doc
//! `docs/design/2026-08-31-buffered-foreach-computed-envs-required-params.md`,
//! Phases 3 and 4. The first four tests are Phase 3's: the load-time
//! validation and the additive display-order map, neither of which changes what
//! prints when. Everything under the Phase 4 banner below pins the emission
//! order itself.

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

// =========================================================================
// Phase 4: buffered capture and ordered replay
//
// Every assertion below is a property, not a fixed transcript: no line of one
// item appears between two lines of another, and the blocks appear in foreach
// item order. The design doc's Acceptance Criteria say so explicitly, because
// the interleaving that main produces is nondeterministic run to run.
// =========================================================================

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Drop ANSI colour escapes so a line's `[task]` prefix can be matched
/// literally. otto colours task prefixes whenever `colored` decides to, which
/// is not something a test should depend on either way.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // CSI ... final byte in @-~. Every escape otto emits is this shape.
        for c in chars.by_ref() {
            if ('@'..='~').contains(&c) && c != '[' {
                break;
            }
        }
    }
    out
}

/// Run otto with its stdout AND stderr pointing at one file, and hand back what
/// the file holds: a faithful merged view, the way both streams land on one
/// terminal. `Output`'s two separate pipes cannot show a cross-stream split.
fn otto_merged(home: &Path, ottofile: &Path, args: &[&str]) -> String {
    let capture = home.join("merged.txt");
    let file = fs::File::create(&capture).unwrap();
    let mut cmd = common::otto_std_cmd(home);
    // One open file description shared by both descriptors, so the two streams
    // share a file offset and their writes are ordered against each other.
    let status = cmd
        .arg("-o")
        .arg(ottofile)
        .args(args)
        .stdout(std::process::Stdio::from(file.try_clone().unwrap()))
        .stderr(std::process::Stdio::from(file))
        .status()
        .unwrap();
    let merged = fs::read_to_string(&capture).unwrap();
    assert!(
        status.code().is_some(),
        "otto must exit normally, not by signal:\n{merged}"
    );
    merged
}

/// Which task each output line belongs to, from its `[task]` prefix. `None` for
/// a line otto did not prefix (an error report, a bare continuation).
fn line_owner(line: &str) -> Option<&str> {
    line.strip_prefix('[')?.split_once(']').map(|(name, _)| name)
}

/// Assert the item-order property: for each item, the lines it owns form one
/// unbroken run, and the runs appear in the order `items` lists them.
///
/// This is the assertion that fails if the replay cursor is reverted to
/// completion order, and the one that fails if any of the seven terminal-writing
/// sites drops the output lock.
fn assert_contiguous_blocks_in_item_order(text: &str, items: &[&str]) {
    let clean = strip_ansi(text);
    let owners: Vec<Option<&str>> = clean.lines().map(line_owner).collect();

    let mut spans = Vec::new();
    for item in items {
        let positions: Vec<usize> = owners
            .iter()
            .enumerate()
            .filter_map(|(i, owner)| (*owner == Some(*item)).then_some(i))
            .collect();
        assert!(!positions.is_empty(), "no output at all for '{item}':\n{clean}");
        let first = positions[0];
        let last = positions[positions.len() - 1];
        assert_eq!(
            positions.len(),
            last - first + 1,
            "'{item}' block is split by another writer's line:\n{clean}"
        );
        spans.push((first, last, *item));
    }

    for pair in spans.windows(2) {
        let (_, prev_end, prev) = pair[0];
        let (next_start, _, next) = pair[1];
        assert!(
            prev_end < next_start,
            "blocks are out of item order: '{next}' starts before '{prev}' ends:\n{clean}"
        );
    }
}

/// Success criterion (a): three items under `parallel: true, buffer: true`
/// print all of alpha's lines, then all of beta's, then all of gamma's, with
/// zero interleaving.
#[test]
fn test_buffered_blocks_print_in_item_order_with_no_interleaving() {
    let temp_dir = TempDir::new().unwrap();
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
      buffer: true
    bash: |
      for i in 1 2 3; do
        echo "${item} line $i"
        sleep 0.05
      done
"#,
    );

    let output = otto(&ottofile, &["say"]);
    assert!(output.status.success(), "the run must succeed: {}", stderr(&output));
    let out = stdout(&output);
    assert_contiguous_blocks_in_item_order(&out, &["say:alpha", "say:beta", "say:gamma"]);

    // Within a block the lines keep their own order, and the status line rides
    // along at the end of the block rather than arriving at completion time.
    let clean = strip_ansi(&out);
    let alpha: Vec<&str> = clean
        .lines()
        .filter(|line| line_owner(line) == Some("say:alpha"))
        .collect();
    assert_eq!(
        alpha,
        vec![
            "[say:alpha] alpha line 1",
            "[say:alpha] alpha line 2",
            "[say:alpha] alpha line 3",
            "[say:alpha] finished successfully",
        ],
        "the status line travels with the block:\n{clean}"
    );
}

/// Success criterion (b): concurrency is proved by a barrier, not a stopwatch.
///
/// Every item announces itself and then waits for all three announcements. If
/// buffering serialized execution, the first item would wait forever and the
/// task body would fail on its own timeout; the run completes only if all three
/// overlap.
#[test]
fn test_buffering_does_not_serialize_execution() {
    let temp_dir = TempDir::new().unwrap();
    let rendezvous = temp_dir.path().join("rendezvous");
    fs::create_dir_all(&rendezvous).unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        &format!(
            r#"
otto:
  api: 1
  envs:
    RENDEZVOUS: {}

tasks:
  say:
    foreach:
      items: [alpha, beta, gamma]
      as: item
      parallel: true
      buffer: true
    bash: |
      touch "${{RENDEZVOUS}}/${{item}}"
      waited=0
      while [ "$(ls "${{RENDEZVOUS}}" | wc -l)" -lt 3 ]; do
        sleep 0.1
        waited=$((waited + 1))
        if [ "$waited" -gt 100 ]; then
          echo "${{item}} never met the others: buffering serialized the group" >&2
          exit 1
        fi
      done
      echo "${{item}} met the others"
"#,
            rendezvous.display()
        ),
    );

    let output = otto(&ottofile, &["say"]);
    assert!(
        output.status.success(),
        "all three items must be live at once: {}",
        stderr(&output)
    );
    assert_contiguous_blocks_in_item_order(&stdout(&output), &["say:alpha", "say:beta", "say:gamma"]);
}

/// Success criterion (c): a failing middle item still prints its block, and the
/// parent still exits non-zero. Buffering changes when bytes print, never what
/// the run's result is.
#[test]
fn test_a_failing_middle_item_still_prints_its_block() {
    let temp_dir = TempDir::new().unwrap();
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
      buffer: true
    bash: |
      echo "${item} out"
      if [ "${item}" = "beta" ]; then exit 3; fi
"#,
    );

    let output = otto(&ottofile, &["say"]);
    assert!(!output.status.success(), "a failed subtask must fail the run");
    let out = stdout(&output);
    assert_contiguous_blocks_in_item_order(&out, &["say:alpha", "say:beta", "say:gamma"]);
    let clean = strip_ansi(&out);
    assert!(
        clean.contains("[say:beta] beta out"),
        "beta's block is printed:\n{clean}"
    );
    assert!(
        strip_ansi(&stderr(&output)).contains("[say:beta] failed"),
        "beta's failure line travels with its block, on stderr as it does unbuffered"
    );
}

/// Success criterion (d): a chatty unbuffered task AND a task that emits skip
/// and status lines run alongside the buffered group, and no replayed block
/// contains a line from either.
///
/// Asserted against a MERGED capture, with otto's stdout and stderr pointing at
/// one file, because that is what a terminal is: a block split by a line on the
/// other stream is split for the reader even though each pipe on its own looks
/// contiguous. The chatty task writes to stderr while the buffered blocks
/// replay to stdout, which is exactly the interleaving the process-wide lock
/// exists to stop; Rust's own per-handle stdout lock cannot, since the two
/// streams have different ones.
#[test]
fn test_no_other_writer_can_split_a_replayed_block() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  chatty:
    bash: |
      # On stderr, and still writing while the buffered blocks replay to stdout.
      for i in $(seq 1 1000); do
        echo "chatty line $i" >&2
        sleep 0.001
      done

  gate:
    bash: exit 1

  gated:
    before: [gate]
    bash: echo unreachable

  say:
    foreach:
      items: [alpha, beta, gamma]
      as: item
      parallel: true
      buffer: true
    bash: |
      # Long enough that replaying the block takes real time under the lock.
      for i in $(seq 1 12000); do echo "${item} line $i"; done

  all:
    before: [chatty, say, gated]
    bash: echo all
"#,
    );

    let merged = otto_merged(temp_dir.path(), &ottofile, &["-j", "8", "all"]);
    let clean = strip_ansi(&merged);
    assert!(
        clean.contains("[chatty] chatty line 1000"),
        "the chatty task must actually have run"
    );
    assert!(
        clean.lines().any(|line| line_owner(line) == Some("gated")),
        "the skip-line emitter must actually have run"
    );
    assert_contiguous_blocks_in_item_order(&merged, &["say:alpha", "say:beta", "say:gamma"]);
}

/// Success criterion (e): a group whose FIRST item is skipped still emits the
/// later items' blocks, in order, without stalling.
///
/// The up-to-date skip is one of the two terminal transitions that send no
/// `TaskReport` at all, so a cursor hung on the report channel would hold beta
/// and gamma behind alpha forever.
#[test]
fn test_a_skipped_first_item_does_not_stall_the_group() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("out")).unwrap();
    for item in ["alpha", "beta", "gamma"] {
        fs::write(root.join("src").join(format!("{item}.txt")), "in\n").unwrap();
    }
    // Only alpha's output already exists, so only alpha is up to date.
    fs::write(root.join("out").join("alpha.txt"), "done\n").unwrap();

    let ottofile = write_ottofile(
        root,
        r#"
otto:
  api: 1

tasks:
  say:
    foreach:
      items: [alpha, beta, gamma]
      as: item
      parallel: true
      buffer: true
    input: ["src/${item}.txt"]
    output: ["out/${item}.txt"]
    bash: |
      echo "${item} ran"
      echo done > out/${item}.txt
"#,
    );

    // `-C`: the `input`/`output` globs resolve against the process cwd, which
    // for a cargo test is the repo root, not the fixture.
    let root_arg = root.to_string_lossy().to_string();
    let output = otto(&ottofile, &["-C", &root_arg, "say"]);
    assert!(output.status.success(), "the run must succeed: {}", stderr(&output));
    let out = stdout(&output);
    assert_contiguous_blocks_in_item_order(&out, &["say:alpha", "say:beta", "say:gamma"]);
    let clean = strip_ansi(&out);
    assert!(
        clean.contains("[say:alpha] skipped (up to date)"),
        "alpha is skipped in position, not dropped:\n{clean}"
    );
    assert!(clean.contains("[say:beta] beta ran"), "beta still runs:\n{clean}");
    assert!(clean.contains("[say:gamma] gamma ran"), "gamma still runs:\n{clean}");
}

/// The other no-report skip path: `mark_skipped`, reached here by a failed
/// prerequisite that makes every subtask unreachable. The blocks are empty, but
/// each item still occupies its slot and the group does not stall.
#[test]
fn test_a_gated_out_group_still_emits_a_line_per_item_in_order() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  gate:
    bash: exit 1

  say:
    before: [gate]
    foreach:
      items: [alpha, beta, gamma]
      as: item
      parallel: true
      buffer: true
    bash: echo "${item} ran"
"#,
    );

    let output = otto(&ottofile, &["say"]);
    assert!(!output.status.success(), "a failed prerequisite must fail the run");
    let out = stdout(&output);
    assert_contiguous_blocks_in_item_order(&out, &["say:alpha", "say:beta", "say:gamma"]);
    let clean = strip_ansi(&out);
    for item in ["alpha", "beta", "gamma"] {
        assert!(
            clean.contains(&format!("[say:{item}] skipped")),
            "'{item}' must occupy its slot:\n{clean}"
        );
        assert!(
            !clean.contains(&format!("{item} ran")),
            "'{item}' never ran, so it has no output:\n{clean}"
        );
    }
}

/// `--no-prefix` still strips the per-line prefix inside a buffered block: one
/// rule for both modes.
#[test]
fn test_no_prefix_strips_prefixes_inside_a_replayed_block() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  say:
    foreach:
      items: [alpha, beta]
      as: item
      parallel: true
      buffer: true
    bash: echo "${item} line"
"#,
    );

    let output = otto(&ottofile, &["--no-prefix", "say"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let clean = strip_ansi(&stdout(&output));
    let lines: Vec<&str> = clean.lines().filter(|line| !line.is_empty()).collect();
    assert!(
        !lines.iter().any(|line| line.starts_with('[')),
        "--no-prefix leaves no [task] prefix anywhere:\n{clean}"
    );
    let alpha = lines.iter().position(|line| *line == "alpha line").expect("alpha ran");
    let beta = lines.iter().position(|line| *line == "beta line").expect("beta ran");
    assert!(alpha < beta, "blocks still emit in item order:\n{clean}");
}

/// Success criterion (g): a subtask that leaves a background child holding the
/// stdout pipe open trips the drain timeout for real, and its block ends with
/// the truncation marker instead of a clean success line over a short block.
///
/// `OUTPUT_PROCESSING_TIMEOUT_SECS` is a hard-coded const with no injection
/// point, so the condition is provoked rather than the timeout shortened; the
/// run therefore takes about that long.
#[test]
fn test_a_failed_drain_ends_the_block_with_a_truncation_marker() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  say:
    foreach:
      items: [alpha, beta]
      as: item
      parallel: true
      buffer: true
    bash: |
      if [ "${item}" = "alpha" ]; then
        sleep 30 2>/dev/null &
      fi
      echo "${item} hi"
"#,
    );

    let output = otto(&ottofile, &["say"]);
    assert!(
        output.status.success(),
        "the process itself exited 0: {}",
        stderr(&output)
    );
    let err = strip_ansi(&stderr(&output));
    assert!(
        err.contains("say:alpha stdout output may be truncated"),
        "alpha's block must say its stdout may be short:\n{err}"
    );
    assert!(
        err.contains("output processing timed out"),
        "the marker names the condition:\n{err}"
    );
    assert!(
        err.contains("say:alpha/stdout.log"),
        "the marker names the log path:\n{err}"
    );
    assert!(
        !err.contains("say:beta stdout output may be truncated"),
        "beta drained cleanly and must carry no marker:\n{err}"
    );
    assert_contiguous_blocks_in_item_order(&stdout(&output), &["say:alpha", "say:beta"]);
}

/// Requesting one item of a buffered foreach prints its output.
///
/// `buffer: true` is a property of the *group*, and ordered replay is the only
/// path to the terminal for a task the replay cursor owns. Asking one item for
/// it by name puts no group in the run, so the cursor owns nothing and no
/// replay ever happens - but the subtask still carried `buffered: true` and
/// suppressed itself anyway. `otto say:alpha` printed its status line and
/// nothing else, at exit 0: the body ran and its output was discarded.
#[test]
fn test_requesting_one_item_of_a_buffered_foreach_prints_its_output() {
    let temp_dir = TempDir::new().unwrap();
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
      buffer: true
    bash: echo "HELLO ${item}"
"#,
    );

    let output = otto(&ottofile, &["say:alpha"]);
    assert!(
        output.status.success(),
        "asking for one item must succeed: {}",
        stderr(&output)
    );
    let clean = strip_ansi(&stdout(&output));
    assert!(
        clean.contains("HELLO alpha"),
        "the requested item's output must reach the terminal:\n{clean}"
    );
    assert!(
        !clean.contains("HELLO beta") && !clean.contains("HELLO gamma"),
        "only the requested item runs:\n{clean}"
    );
}

/// The whole group still replays in item order when the group IS in the run,
/// which is what says the suppression moved rather than went away.
#[test]
fn test_asking_for_the_parent_still_buffers_the_whole_group() {
    let temp_dir = TempDir::new().unwrap();
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
      buffer: true
    bash: echo "HELLO ${item}"
"#,
    );

    let output = otto(&ottofile, &["say"]);
    assert!(output.status.success(), "the run must succeed: {}", stderr(&output));
    assert_contiguous_blocks_in_item_order(&stdout(&output), &["say:alpha", "say:beta", "say:gamma"]);
}
