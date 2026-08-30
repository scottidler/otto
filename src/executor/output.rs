use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use eyre::Result;
use serde::{Deserialize, Serialize};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::broadcast,
};

use super::colors::colorize_task_prefix;

/// Capacity of a task's output broadcast channel. Large because a chatty task can
/// emit thousands of lines before a slow subscriber (the TUI) drains them, and a
/// lagged broadcast subscriber loses messages rather than blocking the producer.
const TASK_OUTPUT_CHANNEL_CAPACITY: usize = 10_000;

/// Type of output stream
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OutputType {
    /// Standard output
    Stdout,
    /// Standard error
    Stderr,
}

/// A single line of task output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOutput {
    /// Name of the task that produced this output
    pub task_name: String,
    /// Type of output stream
    pub stream_type: OutputType,
    /// When this output was produced
    pub timestamp: SystemTime,
    /// The actual output content
    pub content: String,
}

/// Status of a task for TUI display
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TuiTaskStatus {
    /// Task is waiting to be executed
    Pending,
    /// Task is currently running
    Running,
    /// Task completed successfully
    Completed,
    /// Task failed with an error
    Failed,
    /// Task was skipped
    Skipped,
}

/// Message sent from scheduler to TUI with task updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskMessage {
    /// Task output line
    Output(TaskOutput),
    /// Task status change
    StatusChange {
        task_name: String,
        status: TuiTaskStatus,
        timestamp: SystemTime,
    },
    /// Task started executing
    Started { task_name: String, timestamp: SystemTime },
    /// Task finished executing
    Finished {
        task_name: String,
        status: TuiTaskStatus,
        timestamp: SystemTime,
        duration_ms: u64,
    },
}

/// A writer that writes to both a file and a terminal
pub struct TeeWriter {
    /// File to write to
    file: File,
    /// Whether this is stderr (true) or stdout (false)
    is_stderr: bool,
    /// Task name for prefixing output
    task_name: String,
    /// Whether to suppress terminal output (for TUI mode)
    suppress_terminal: bool,
    /// Whether to omit the `[task]` prefix from terminal output (`--no-prefix`,
    /// see docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md Phase 8).
    /// File output is never prefixed either way, so this only changes what
    /// reaches the terminal.
    no_prefix: bool,
}

/// Build the bytes that go to the terminal for one chunk of task output:
/// the colored `[task]` prefix ahead of the data, or the data alone when
/// `no_prefix` (`--no-prefix`) is set. Pure so the prefix-suppression logic
/// is unit-testable without capturing real process stdout/stderr.
fn format_terminal_output(task_name: &str, data: &[u8], no_prefix: bool) -> String {
    if no_prefix {
        String::from_utf8_lossy(data).to_string()
    } else {
        let colored_prefix = colorize_task_prefix(task_name);
        format!("{} {}", colored_prefix, String::from_utf8_lossy(data))
    }
}

impl TeeWriter {
    pub async fn new(file: File, is_stderr: bool, task_name: String, suppress_terminal: bool, no_prefix: bool) -> Self {
        Self {
            file,
            is_stderr,
            task_name,
            suppress_terminal,
            no_prefix,
        }
    }

    pub async fn write(&mut self, data: &[u8]) -> Result<()> {
        // Always write to file (no colors)
        self.file.write_all(data).await?;

        // Conditionally write to terminal (suppressed in TUI mode)
        if !self.suppress_terminal {
            // Write to terminal, with or without the colored task name prefix
            let terminal_output = format_terminal_output(&self.task_name, data, self.no_prefix);
            if self.is_stderr {
                eprint!("{terminal_output}");
            } else {
                print!("{terminal_output}");
            }

            // Ensure terminal output is flushed
            if self.is_stderr {
                io::stderr().flush()?;
            } else {
                io::stdout().flush()?;
            }
        }

        Ok(())
    }
}

/// Manages output streams for a task
#[derive(Debug, Clone)]
pub struct TaskStreams {
    /// Path to stdout log file
    pub stdout_file: PathBuf,
    /// Path to stderr log file
    pub stderr_file: PathBuf,
    /// Broadcast channel for real-time output
    pub output_tx: broadcast::Sender<TaskOutput>,
}

impl TaskStreams {
    pub async fn new(task_name: &str, output_dir: &Path) -> Result<Self> {
        let task_dir = output_dir.join(task_name);
        if !task_dir.exists() {
            tokio::fs::create_dir_all(&task_dir).await?;
        }

        let stdout_file = task_dir.join("stdout.log");
        let stderr_file = task_dir.join("stderr.log");

        File::create(&stdout_file).await?;
        File::create(&stderr_file).await?;

        let (output_tx, _) = broadcast::channel(TASK_OUTPUT_CHANNEL_CAPACITY);

        Ok(Self {
            stdout_file,
            stderr_file,
            output_tx,
        })
    }

    pub async fn process_output(
        &self,
        task_name: String,
        output_type: OutputType,
        mut reader: impl AsyncBufReadExt + Unpin,
        suppress_terminal: bool,
        no_prefix: bool,
    ) -> Result<()> {
        let output_file = match output_type {
            OutputType::Stdout => &self.stdout_file,
            OutputType::Stderr => &self.stderr_file,
        };

        let file = File::create(output_file).await?;
        let mut writer = TeeWriter::new(
            file,
            matches!(output_type, OutputType::Stderr),
            task_name.clone(),
            suppress_terminal,
            no_prefix,
        )
        .await;

        let mut line = String::new();

        while let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 {
                break;
            }

            let output = TaskOutput {
                task_name: task_name.clone(),
                stream_type: output_type.clone(),
                timestamp: SystemTime::now(),
                content: line.clone(),
            };

            // Write to both file and terminal
            writer.write(line.as_bytes()).await?;

            // Broadcast for real-time monitoring
            let _ = self.output_tx.send(output);

            line.clear();
        }

        Ok(())
    }

    pub async fn read_output(&self, output_type: OutputType) -> Result<Vec<String>> {
        let file_path = match output_type {
            OutputType::Stdout => &self.stdout_file,
            OutputType::Stderr => &self.stderr_file,
        };

        let mut lines = Vec::new();
        let file = File::open(file_path).await?;
        let mut reader = BufReader::new(file).lines();

        while let Some(line) = reader.next_line().await? {
            lines.push(line);
        }

        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
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
}
