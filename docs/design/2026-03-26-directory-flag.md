# Design Document: `-C` / `--cwd` Flag

**Author:** Scott Idler
**Date:** 2026-03-26
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Add a `-C <dir>` / `--cwd <dir>` flag to otto that changes the working directory before doing anything else. This mirrors the behavior of `make -C`, `git -C`, and `cargo`'s `--manifest-path` pattern, letting users run otto against a project without `cd`-ing into it first.

## Problem Statement

### Background

Otto discovers its ottofile by walking up from the current working directory. All path resolution - ottofile discovery, foreach globs, workspace root, task execution directory - flows from the cwd captured in `Parser::new()` via `env::current_dir()`.

The existing `-o`/`--ottofile` flag lets users point to a specific ottofile, but it doesn't change the working directory for task execution. Users who want to run otto against a different project must `cd` first or use subshells (`(cd /other/project && otto ci)`).

### Problem

There's no way to run `otto ci` against a different project directory from the current shell without changing directory. This is inconvenient when:

- Running otto from CI scripts that operate across multiple repos
- Using tools like Claude Code that invoke `otto` with `--manifest-path`-style patterns
- Running quick checks from a parent directory (`otto -C some-service ci`)

### Goals

- Add `-C <dir>` flag that changes the effective working directory before any other processing
- Compose cleanly with existing flags (`-o`, `-j`, `-t`)
- Follow the precedent set by `make -C` and `git -C`

### Non-Goals

- Supporting multiple `-C` flags that chain (like `git -C a -C b`) - one is enough
- Adding a `OTTO_CWD` environment variable (can be added later if wanted)
- Changing how `-o`/`--ottofile` works

## Proposed Solution

### Overview

Add a `-C`/`--cwd` argument to `otto_command()`. Before `Parser::new()` captures the cwd, extract `-C` from argv and call `env::set_current_dir()`. This way every downstream consumer - `Parser::new()`, `divine_ottofile()`, `Workspace::new()`, task execution - sees the correct directory without any changes.

### Architecture

The change is isolated to two files:

1. **`src/main.rs`** - Pre-parse `-C` from raw args, call `set_current_dir()` before `Parser::new()`
2. **`src/cli/parser.rs`** - Add `-C`/`--cwd` to `otto_command()` so it appears in `--help` output and clap doesn't reject it

The key insight: by changing the process working directory early in `main()`, all existing code paths that use `env::current_dir()` or relative paths automatically do the right thing. No changes needed in parser, workspace, scheduler, or ottofile discovery.

**Important detail:** The pre-parse must also strip `-C` and its value from the args vector before passing args to `handle_subcommand()` and `Parser::new()`. Without stripping, two problems arise:

1. `handle_subcommand()` checks `args[1]` for subcommand names like "Clean". If the user writes `otto -C /dir Clean`, `args[1]` is `-C` and the subcommand is missed.
2. Clap would also try to consume the arg. While we register it in clap for `--help`, having it consumed twice (pre-parse + clap) would be confusing. Better to strip it and let clap never see it.

### Implementation Plan

#### Phase 1: Pre-parse and strip in main.rs

Add a function that scans raw args for `-C <dir>` or `--cwd <dir>` or `--cwd=<dir>`, validates the directory exists, calls `env::set_current_dir()`, and returns the args with `-C` and its value removed.

```rust
fn apply_directory_flag(args: Vec<String>) -> Result<Vec<String>, Report> {
    let mut i = 0;
    let mut dir_value: Option<String> = None;
    let mut skip_indices: Vec<usize> = Vec::new();

    while i < args.len() {
        let arg = &args[i];
        // Stop scanning at "--" (end of flags)
        if arg == "--" {
            break;
        }
        if arg == "-C" || arg == "--cwd" {
            skip_indices.push(i);
            if let Some(d) = args.get(i + 1) {
                dir_value = Some(d.clone());
                skip_indices.push(i + 1);
                i += 2;
            } else {
                eyre::bail!("'{}' requires a directory argument", arg);
            }
        } else if let Some(d) = arg.strip_prefix("--cwd=") {
            dir_value = Some(d.to_string());
            skip_indices.push(i);
            i += 1;
        } else {
            i += 1;
        }
    }

    // If multiple -C flags found, the last one wins (same behavior as the loop above).
    // Could error here instead, but "last wins" is simpler and matches how most CLI tools
    // handle repeated flags.
    if let Some(dir) = dir_value {
        let path = PathBuf::from(&dir);
        if !path.is_dir() {
            eyre::bail!("directory '{}' does not exist or is not a directory", dir);
        }
        env::set_current_dir(&path)
            .wrap_err_with(|| format!("failed to change directory to '{}'", dir))?;
    }

    let filtered: Vec<String> = args
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !skip_indices.contains(i))
        .map(|(_, a)| a)
        .collect();

    Ok(filtered)
}
```

Call it in `main()` right after collecting args, before anything that reads them:

```rust
let args: Vec<String> = env::args().collect();
let args = match apply_directory_flag(args) {
    Ok(a) => a,
    Err(e) => {
        eprintln!("{e}");
        std::process::exit(1);
    }
};
```

This ensures `handle_is_valid_ottofile`, `handle_subcommand`, and `Parser::new` all see clean args and the correct cwd.

#### Phase 2: Register in clap (help only)

Add the arg to `otto_command()` so it appears in `--help` output. Since the arg is already stripped from argv before clap sees it, this is purely for documentation. We still register it so that if someone passes `--help`, they see `-C` listed.

Note: since `-C` is stripped before clap parses, clap will never actually see it. The registration is only for `--help` text. We could alternatively add it to a custom help string, but registering it in clap keeps all flags in one place.

```rust
.arg(
    Arg::new("cwd")
        .short('C')
        .long("cwd")
        .value_name("DIR")
        .help("Change to DIR before doing anything")
        .value_parser(value_parser!(String)),
)
```

#### Phase 3: Tests

- Unit test: `apply_directory_flag` with valid dir changes cwd and strips `-C` from args
- Unit test: `apply_directory_flag` with nonexistent dir returns error
- Unit test: `apply_directory_flag` with no flag returns args unchanged
- Unit test: `apply_directory_flag` with `--cwd=<dir>` form works
- Unit test: `apply_directory_flag` with missing dir value after `-C` returns error
- Integration: `otto -C <tempdir-with-ottofile> --list-subtasks` finds tasks in target dir

## Alternatives Considered

### Alternative 1: Override cwd inside Parser

- **Description:** Pass the `-C` value into `Parser::new()` and override `self.cwd` there instead of calling `set_current_dir()`.
- **Pros:** Doesn't mutate global process state.
- **Cons:** `Workspace::new(cwd)` in `app.rs` also reads `env::current_dir()` independently. Would need to thread the override through multiple layers. `divine_ottofile()` uses `fs::canonicalize()` which resolves relative to process cwd. Task execution uses `current_dir(workspace.root())`. Every consumer would need the override.
- **Why not chosen:** Far more invasive for no practical benefit. Otto is a single-threaded-at-startup CLI - mutating process cwd is safe and idiomatic (make, git, cargo all do this).

### Alternative 2: Use only `--ottofile`

- **Description:** Tell users to use `-o /path/to/project` which already changes the base directory for ottofile resolution.
- **Pros:** No code changes.
- **Cons:** `-o` changes where the ottofile is found but doesn't change the workspace root or task execution directory. Tasks would still run in the original cwd. Users would need to understand the subtle difference. Doesn't solve the actual problem.
- **Why not chosen:** Semantically different from "run in this directory."

## Technical Considerations

### Dependencies

No new runtime dependencies. Uses only `std::env::set_current_dir` and `std::path::PathBuf`. Testing may benefit from `serial_test` crate for `#[serial]` if not already present.

### Performance

Zero impact. A single `chdir` syscall at startup.

### Security

The directory must exist and be accessible. Standard filesystem permission checks apply via `set_current_dir()`.

### Testing Strategy

- Unit tests for the pre-parse function using `tempfile::TempDir`
- Since `set_current_dir` is process-global, unit tests that call it must not run in parallel with other cwd-sensitive tests (use `#[serial]` or run in separate test binaries)
- Integration test: create a temp dir with an ottofile, run `otto -C <dir> --list-subtasks` and verify it finds tasks

### Rollout Plan

Single commit. No migration, no config changes, no breaking changes. The flag is purely additive.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `set_current_dir` affects subcommands (Clean, History, etc.) | Low | Low | Subcommands also benefit from `-C`; they use the same cwd-based discovery |
| `-C` conflicts with a future flag | Low | Low | `-C` is the established convention (make, git, ninja); unlikely to need it for something else |
| Pre-parse doesn't handle all arg formats | Low | Medium | Test `-C dir`, `--cwd dir`, `--cwd=dir` explicitly. Skip `-Cdir` (no space) - clap doesn't support this for short args with values either |
| Test isolation with `set_current_dir` | Medium | Low | Use `#[serial]` from `serial_test` crate or isolate into integration tests |
| `-C` value consumed as task name (e.g., `otto ci -C /dir`) | Low | Medium | Pre-parse scans all args, not just leading ones, so position doesn't matter. A task arg that matches `-C` is unlikely. |
| Relative `-C` path resolves against original cwd | Low | Low | This is correct behavior - `set_current_dir("../other")` resolves relative to where the user is |

## Open Questions

- [x] Should `-C` also apply to the built-in subcommands (Clean, History, Stats, Upgrade)? Yes - the pre-parse approach means it automatically does, which is the correct behavior.
- [ ] Add `OTTO_CWD` env var support? Can defer to a follow-up.

## References

- `make -C dir` - [GNU Make manual](https://www.gnu.org/software/make/manual/make.html)
- `git -C dir` - changes directory before doing anything
- Current ottofile discovery: `src/cli/parser.rs:1896-1922`
- Workspace root setup: `src/app.rs:289-290`
