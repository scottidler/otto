//! Shared home-isolation helper for integration tests.
//!
//! `docs/design/2026-06-10-code-review-remediation.md`, Phase 11: a full
//! `cargo test` with neither `OTTO_HOME` nor `OTTO_DB_PATH` set used to write
//! a project row per temp-dir fixture into the developer's real
//! `~/.otto/otto.db` (1040 of 1264 rows on the live db were `.tmp*` test
//! artifacts, measured 2026-08-29). Every integration test that spawns the
//! `otto` binary or builds a `StateManager` must isolate its home through
//! this module instead of rolling its own `env(...)` calls, so the isolation
//! can't drift file by file.
//!
//! `StateManager::default_db_path()` (`src/executor/state/db.rs`) checks
//! `$OTTO_DB_PATH` first, then falls back to `resolve_otto_home()`
//! (`src/executor/layout.rs`), which checks `$OTTO_HOME` before falling back
//! to `$HOME/.otto`. Setting `HOME` alone, without also pinning `OTTO_HOME`
//! and removing `OTTO_DB_PATH`, is not isolation: a developer's shell that
//! exports either one would still steer the child at the real store.
//!
//! Three shapes, because a test's spawn shape varies and the isolation must
//! not: [`otto_cmd`] for an `assert_cmd::Command`, [`otto_std_cmd`] for the
//! tests that need raw stdio piping, and [`isolate`] for the pty tests that
//! reach otto indirectly through `script`. No `tests/*.rs` file constructs a
//! command from `CARGO_BIN_EXE_otto` or `cargo_bin_cmd!` on its own.
//!
//! Not every test file that includes this module uses every item in it;
//! that's expected, since each `tests/*.rs` file is its own compiled binary.

#![allow(dead_code)]

use assert_cmd::cargo::cargo_bin_cmd;
use std::path::Path;

/// Build an `otto` command isolated to `home`.
///
/// `OTTO_HOME` is pinned to `home` and `OTTO_DB_PATH` is removed from the
/// child's environment, so the spawned binary cannot read or write the
/// developer's real database no matter what the ambient environment
/// exports. Pinned by `tests/db_isolation_test.rs`.
pub fn otto_cmd(home: &Path) -> assert_cmd::Command {
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.env("OTTO_HOME", home).env_remove("OTTO_DB_PATH");
    cmd
}

/// The `otto` binary under test, for the two shapes `assert_cmd` cannot
/// express: raw stdio piping, and running otto *indirectly* (under `script`
/// for a pty). Always pair it with [`isolate`].
pub const OTTO_BIN: &str = env!("CARGO_BIN_EXE_otto");

/// The `std::process` twin of [`otto_cmd`], for tests that need raw stdio
/// piping (`Stdio::piped`, writing to the child's stdin), which
/// `assert_cmd::Command` does not expose. Same isolation, same two env calls.
pub fn otto_std_cmd(home: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(OTTO_BIN);
    isolate(&mut cmd, home);
    cmd
}

/// Apply [`otto_cmd`]'s isolation to a command that reaches otto *indirectly*,
/// such as `script -qec "<otto ...>"` in the pty tests, where the binary is
/// spawned by the shell `script` starts and inherits `script`'s environment.
pub fn isolate<'c>(cmd: &'c mut std::process::Command, home: &Path) -> &'c mut std::process::Command {
    cmd.env("OTTO_HOME", home).env_remove("OTTO_DB_PATH")
}

/// Build a `StateManager` rooted at `home`'s `otto.db`, for tests that talk
/// to the store directly rather than through the binary.
pub fn isolated_state_manager(home: &Path) -> otto::executor::state::StateManager {
    otto::executor::state::StateManager::with_db_path(home.join("otto.db"))
        .expect("failed to open scratch StateManager")
}

/// Build a `script` invocation that runs `argv` under a real pty, in the
/// dialect the host's `script` actually speaks.
///
/// The two `script` implementations share a name and nothing else:
///
/// - util-linux (Linux): `script -qec "<one shell string>" /dev/null`
/// - BSD (macOS): `script -q /dev/null <program> <args...>`
///
/// Every pty test here hardcoded the util-linux form, so on macOS `script`
/// rejected the arguments, produced no output, and the tests failed on their
/// own vacuous-pass guards ("the child never entered the alternate screen").
/// That is the guards working: the pty was never allocated. The dialect is the
/// only thing that was wrong.
///
/// util-linux takes ONE string that a shell parses, so the argv is joined and
/// single-quoted here. BSD takes the argv directly, which needs no quoting.
/// `script` starts the command itself, so the child inherits `script`'s
/// environment: pass the returned command through [`isolate`] when the child
/// is otto.
pub fn pty_cmd(argv: &[&str]) -> std::process::Command {
    let mut cmd = std::process::Command::new("script");
    if cfg!(target_os = "macos") {
        cmd.arg("-q").arg("/dev/null").args(argv);
    } else {
        let joined = argv.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
        cmd.arg("-qec").arg(joined).arg("/dev/null");
    }
    cmd
}

/// Single-quote one argv element for the shell `script -c` hands the string to.
fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// The child's own output from a [`pty_cmd`] run, with the pty's echo stripped.
///
/// BSD `script` forwards its own stdin to the pty, and `cargo test` gives it no
/// terminal, so it reads EOF immediately and the pty echoes the EOF character
/// before the child writes anything: the stream opens with the four bytes
/// `^D\x08\x08`, the terminal's rendering of a control-D it then erases.
/// util-linux `script` does not do this. That prefix is `script` talking, not
/// the program under test, and it made a YAML parse fail at position 2 with
/// "control characters are not allowed" on macOS only.
///
/// `\r` goes too: a pty terminates lines with CRLF on both platforms.
pub fn pty_stdout(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let text = text.strip_prefix("^D").unwrap_or(&text).trim_start_matches('\u{8}');
    text.replace('\r', "")
}
