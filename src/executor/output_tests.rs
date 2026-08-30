#![cfg(test)]

use super::*;

#[tokio::test]
async fn test_output_processing() {
    let temp_dir = tempfile::tempdir().unwrap();
    let output_dir = PathBuf::from(temp_dir.path());

    let streams = TaskStreams::new("test_task", &output_dir).await.unwrap();

    let test_output = "line 1\nline 2\nline 3\n";
    let mut rx = streams.output_tx.subscribe();

    // Process the output
    let mut cursor = std::io::Cursor::new(test_output);
    streams
        .process_output("test_task".to_string(), OutputType::Stdout, &mut cursor, false, false)
        .await
        .unwrap();

    let contents = streams.read_output(OutputType::Stdout).await.unwrap();
    assert_eq!(contents.len(), 3);
    assert_eq!(contents[0], "line 1");

    let received = rx.try_recv().unwrap();
    assert_eq!(received.task_name, "test_task");
    assert_eq!(received.content, "line 1\n");
}

/// A non-UTF-8 byte mid-stream used to end the drain: `read_line`'s
/// InvalidData error was read as EOF, so the terminal and `stdout.log` both
/// stopped at line 1 and the task still reported success.
#[tokio::test]
async fn a_non_utf8_byte_does_not_truncate_the_log() {
    let temp_dir = tempfile::tempdir().unwrap();
    let output_dir = PathBuf::from(temp_dir.path());

    let streams = TaskStreams::new("test_task", &output_dir).await.unwrap();

    let mut test_output: Vec<u8> = Vec::new();
    test_output.extend_from_slice(b"line1\n");
    test_output.extend_from_slice(b"\xff\xfe bad\n");
    test_output.extend_from_slice(b"line3\nline4\nline5\n");

    let mut cursor = std::io::Cursor::new(test_output);
    streams
        .process_output("test_task".to_string(), OutputType::Stdout, &mut cursor, true, false)
        .await
        .unwrap();

    let contents = streams.read_output(OutputType::Stdout).await.unwrap();
    assert_eq!(contents.len(), 5, "every line must survive the bad byte: {contents:?}");
    assert_eq!(contents[0], "line1");
    assert_eq!(contents[4], "line5");
}

#[tokio::test]
async fn test_multiple_streams() {
    let temp_dir = tempfile::tempdir().unwrap();
    let output_dir = PathBuf::from(temp_dir.path());

    let streams = TaskStreams::new("test_task", &output_dir).await.unwrap();

    // Write to both stdout and stderr
    let stdout_data = "stdout line\n";
    let stderr_data = "stderr line\n";

    let mut stdout_cursor = std::io::Cursor::new(stdout_data);
    let mut stderr_cursor = std::io::Cursor::new(stderr_data);

    // Process both streams
    streams
        .process_output(
            "test_task".to_string(),
            OutputType::Stdout,
            &mut stdout_cursor,
            false,
            false,
        )
        .await
        .unwrap();

    streams
        .process_output(
            "test_task".to_string(),
            OutputType::Stderr,
            &mut stderr_cursor,
            false,
            false,
        )
        .await
        .unwrap();

    let stdout_contents = streams.read_output(OutputType::Stdout).await.unwrap();
    let stderr_contents = streams.read_output(OutputType::Stderr).await.unwrap();

    assert_eq!(stdout_contents[0], "stdout line");
    assert_eq!(stderr_contents[0], "stderr line");
}

/// `--no-prefix` (docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md
/// Phase 8): terminal output must drop the `[task]` prefix entirely, not
/// just the color, leaving exactly the task's own bytes.
#[test]
fn test_no_prefix_omits_task_prefix() {
    let out = format_terminal_output("loud-task", b"hello\n", true);
    assert_eq!(out, "hello\n");
}

/// Same call with `no_prefix: false` (the default) still carries the
/// task name, so a future regression that always suppresses the prefix
/// would fail this test.
#[test]
fn test_prefix_present_by_default() {
    let out = format_terminal_output("loud-task", b"hello\n", false);
    assert!(
        out.contains("loud-task"),
        "expected task name in prefixed output: {out:?}"
    );
    assert!(out.contains("hello\n"), "expected data to still be present: {out:?}");
}
