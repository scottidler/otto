use env_logger::Target;
use eyre::{Report, Result, WrapErr};
use log::info;
use otto::cli::Parser;
use otto::{RuntimeConfig, Startup};
use std::env;
use std::fs::OpenOptions;
use std::path::PathBuf;

/// Default maximum log file size before rotation (10 MB).
const DEFAULT_MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

/// Rotate the log file if it exceeds the size threshold.
///
/// Renames `otto.log` to `otto.log.1` (one backup maximum).
/// Threshold can be overridden via `OTTO_MAX_LOG_BYTES` env var.
fn rotate_log_if_needed(log_file_path: &std::path::Path) {
    let max_bytes = std::env::var("OTTO_MAX_LOG_BYTES")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MAX_LOG_BYTES);

    if let Ok(meta) = std::fs::metadata(log_file_path)
        && meta.len() > max_bytes
    {
        let backup = log_file_path.with_extension("log.1");
        // Overwrite any existing backup
        let _ = std::fs::rename(log_file_path, backup);
    }
}

/// The log levels `--log-level` accepts, in the order `--help` lists them.
const LOG_LEVELS: &[&str] = &["off", "error", "warn", "info", "debug", "trace"];

/// Default log filter when neither `--log-level` nor `$RUST_LOG` says otherwise.
const DEFAULT_LOG_LEVEL: &str = "info";

/// Pre-parse `--log-level` from raw args and return it with the flag stripped.
///
/// Pre-parsed rather than left to clap because logging is set up before the
/// parser exists, and because `otto --log-level debug build` has to reach the
/// task parser without the flag in the arg list.
///
/// Case-insensitive per the CLI rule: users type back the `WARN`/`INFO` they
/// saw in the log file.
fn apply_log_level_flag(args: Vec<String>) -> Result<(Vec<String>, Option<String>), Report> {
    let mut level: Option<String> = None;
    let mut kept: Vec<String> = Vec::with_capacity(args.len());
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            kept.extend_from_slice(&args[i..]);
            break;
        }
        if arg == "--log-level" {
            let Some(value) = args.get(i + 1) else {
                eyre::bail!("'--log-level' requires a value; one of: {}", LOG_LEVELS.join(", "));
            };
            level = Some(value.clone());
            i += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--log-level=") {
            level = Some(value.to_string());
            i += 1;
            continue;
        }
        kept.push(arg.clone());
        i += 1;
    }

    let level = match level {
        None => None,
        Some(value) => {
            let lowered = value.to_ascii_lowercase();
            if !LOG_LEVELS.contains(&lowered.as_str()) {
                eyre::bail!(
                    "invalid --log-level '{value}'; expected one of: {}",
                    LOG_LEVELS.join(", ")
                );
            }
            Some(lowered)
        }
    };

    Ok((kept, level))
}

fn setup_logging(level: Option<&str>) -> Result<(), Report> {
    // XDG on every platform (see executor::layout): `dirs::data_local_dir()`
    // ignores $XDG_DATA_HOME off Linux, so otto's logs used to land somewhere
    // its own help text never mentioned.
    let log_dir = otto::executor::layout::log_dir()
        .ok_or_else(|| eyre::eyre!("Could not determine the XDG data directory for otto's logs"))?;

    std::fs::create_dir_all(&log_dir)?;
    let log_file_path = log_dir.join("otto.log");

    // Rotate log file before opening if it's too large
    rotate_log_if_needed(&log_file_path);

    let log_file = OpenOptions::new().create(true).append(true).open(&log_file_path)?;

    // `--log-level` wins over $RUST_LOG: an explicit flag is a decision, an
    // inherited env var is an accident.
    let filter = match level {
        Some(level) => level.to_string(),
        None => env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_LOG_LEVEL.to_string()),
    };

    env_logger::Builder::new()
        .parse_filters(&filter)
        .target(Target::Pipe(Box::new(log_file)))
        .init();

    Ok(())
}

/// Pre-parse `-C`/`--cwd` from raw args, change the process working directory,
/// and return the args with the flag stripped out.
fn apply_cwd_flag(args: Vec<String>) -> Result<Vec<String>, Report> {
    let mut i = 0;
    let mut dir_value: Option<String> = None;
    let mut skip_indices: Vec<usize> = Vec::new();

    while i < args.len() {
        let arg = &args[i];
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
        } else if let Some(d) = arg.strip_prefix("-C=") {
            dir_value = Some(d.to_string());
            skip_indices.push(i);
            i += 1;
        } else {
            i += 1;
        }
    }

    if let Some(dir) = dir_value {
        let path = PathBuf::from(&dir);
        if !path.is_dir() {
            eyre::bail!("directory '{}' does not exist or is not a directory", dir);
        }
        env::set_current_dir(&path).wrap_err_with(|| format!("failed to change directory to '{}'", dir))?;
    }

    let filtered: Vec<String> = args
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !skip_indices.contains(i))
        .map(|(_, a)| a)
        .collect();

    Ok(filtered)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    let (args, log_level) = match apply_log_level_flag(args) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };

    if let Err(e) = setup_logging(log_level.as_deref()) {
        eprintln!("Failed to setup logging: {e:#}");
        std::process::exit(1);
    }
    info!("Starting otto");

    let args = match apply_cwd_flag(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };

    // Handle hidden --is-valid-ottofile arg early (before normal parsing)
    if let Some(exit_code) = handle_is_valid_ottofile(&args) {
        std::process::exit(exit_code);
    }

    // Handle subcommands that use their own clap parsers
    if args.len() > 1
        && let Some(result) = handle_subcommand(&args).await
    {
        if let Err(e) = result {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
        return;
    }

    // Parse and run main command
    let mut parser = match Parser::new(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };

    // The parser reports what to do; this is the only place that ends the
    // process. `Startup::Exit` means help/version/--tasks already printed.
    let config = match RuntimeConfig::from_parser(&mut parser) {
        Ok(Startup::Run(c)) => *c,
        Ok(Startup::Exit(code)) => std::process::exit(code),
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };

    if let Err(e) = otto::run(config).await {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}

/// Handle --is-valid-ottofile argument. Returns Some(exit_code) if handled.
fn handle_is_valid_ottofile(args: &[String]) -> Option<i32> {
    for (i, arg) in args.iter().enumerate() {
        if arg == "--is-valid-ottofile" {
            if let Some(path_arg) = args.get(i + 1) {
                let filename = std::path::Path::new(path_arg)
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or(path_arg);
                if otto::cli::is_valid_ottofile_name(filename) {
                    return Some(0);
                }
            }
            return Some(1);
        } else if let Some(path_arg) = arg.strip_prefix("--is-valid-ottofile=") {
            let filename = std::path::Path::new(path_arg)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(path_arg);
            if otto::cli::is_valid_ottofile_name(filename) {
                return Some(0);
            }
            return Some(1);
        }
    }
    None
}

/// Otto's global flags that consume the following token as their value.
///
/// `-C`/`--cwd` is already stripped by `apply_cwd_flag` and `--log-level` by
/// `apply_log_level_flag`, but both stay listed: this table's job is to say
/// which tokens are values, and a table that is only right because of call
/// ordering is a trap for the next edit.
const GLOBAL_VALUE_FLAGS: &[&str] = &[
    "-C",
    "--cwd",
    "-o",
    "--ottofile",
    "-j",
    "--jobs",
    "--format",
    "--log-level",
];

/// Index of the first argument that names a command rather than a global flag.
///
/// `handle_subcommand` only ever looked at `args[1]`, so any leading global
/// flag defeated builtin routing outright: `otto -j 2 Convert` fell through to
/// the task parser and printed "No tasks to execute".
fn first_command_index(args: &[String]) -> Option<usize> {
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            return None;
        }
        if GLOBAL_VALUE_FLAGS.contains(&arg.as_str()) {
            i += 2;
            continue;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        return Some(i);
    }
    None
}

/// The builtins that run before any ottofile is read, each through its own clap
/// parser.
///
/// A separate enum from `otto::app::Builtin` because it is deliberately not the
/// same set: `Graph` needs the parsed ottofile's task specs, so it has no early
/// route and is reached by the task route alone. Naming the difference in a
/// type is what lets a test assert it, instead of a comment claiming it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EarlyCommand {
    Clean,
    Convert,
    History,
    Stats,
    Upgrade,
}

impl EarlyCommand {
    /// The early route for a command name, if it has one. Pure: no I/O, and it
    /// runs nothing.
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "Clean" => Some(Self::Clean),
            "Convert" => Some(Self::Convert),
            "History" => Some(Self::History),
            "Stats" => Some(Self::Stats),
            "Upgrade" => Some(Self::Upgrade),
            _ => None,
        }
    }
}

/// Handle subcommands that use their own clap parsers. Returns Some(result) if handled.
async fn handle_subcommand(args: &[String]) -> Option<Result<(), Report>> {
    let index = first_command_index(args)?;
    let rest = &args[index..];
    Some(match EarlyCommand::from_name(&rest[0])? {
        EarlyCommand::Clean => otto::app::execute_clean_command(rest).await,
        EarlyCommand::Convert => otto::app::execute_convert_command(rest),
        EarlyCommand::History => otto::app::execute_history_command(rest),
        EarlyCommand::Stats => otto::app::execute_stats_command(rest),
        EarlyCommand::Upgrade => otto::app::execute_upgrade_command(rest).await,
    })
}

#[path = "main_tests.rs"]
mod tests;
