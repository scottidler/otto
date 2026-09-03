//#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::fmt::Debug;
use std::fs;
use std::io::IsTerminal;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::executor::task::TaskEdge;

use clap::{Arg, ArgMatches, Command, value_parser};
use daggy::Dag;
use eyre::{Context, Result, eyre};
use glob;
use hex;
use sha2::{Digest, Sha256};

use crate::cfg::config::{ConfigSpec, ParamSpec, TaskSpec, Value};
use crate::cfg::edge::When;
use crate::cfg::env as env_eval;
use crate::cfg::param::{Nargs, ParamType};
use crate::cfg::resolver::{self, DynamicResolver};
use crate::cfg::task::{ForeachItem, ForeachSpec, TaskSpecs};
use crate::cli::builtins::{BUILTIN_COMMANDS, is_builtin};

pub type DAG<T> = Dag<T, (), u32>;

/// Where otto should look for an ottofile.
///
/// This used to be a `String` carrying `"."` as a sentinel meaning "divine one".
/// It worked only because `"."` also happens to be a real relative path naming
/// the cwd, so every consumer that treated the sentinel as a literal path got
/// the right answer by coincidence. Any consumer that compared it, joined it to
/// another path, or displayed it saw a directory the user never asked for. The
/// two states are different, so they are different variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OttofileSource {
    /// No `-o/--ottofile` and no `$OTTOFILE`: search upward from the cwd.
    Divine,
    /// A path the user named, on the flag or in the environment. A directory is
    /// searched; a file is used as given.
    Explicit(String),
}

impl OttofileSource {
    /// The path to start from, which is the cwd when nothing was named.
    fn as_start_path(&self) -> &str {
        match self {
            Self::Divine => ".",
            Self::Explicit(value) => value,
        }
    }
}

impl std::fmt::Display for OttofileSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Divine => write!(f, "<divined from the current directory>"),
            Self::Explicit(value) => write!(f, "{value}"),
        }
    }
}

/// The values `--log-level` accepts, in increasing verbosity.
///
/// Declared here because `global_args()` renders them in `--help`; `main`
/// pre-parses the flag (logging is configured before a parser exists) and
/// reads the same list, so the two can't drift.
pub const LOG_LEVELS: &[&str] = &["off", "error", "warn", "info", "debug", "trace"];

/// Guard key for `otto.envs-command`'s recursion chain. A fixed literal, not a
/// task name: there is exactly one `envs-command` resolution per invocation.
const ENVS_GUARD_KEY: &str = "otto.envs-command";

/// Error prefix for everything `otto.envs-command` can fail at, so a message
/// names the key the user wrote rather than an internal function.
const ENVS_COMMAND_CONTEXT: &str = "otto.envs-command";

const OTTOFILES: &[&str] = &[
    "otto.yml",
    ".otto.yml",
    "otto.yaml",
    ".otto.yaml",
    "Ottofile",
    "OTTOFILE",
];

/// Returns the base directory for resolving paths declared relative to an
/// ottofile - workspace root for task execution, anchor for foreach globs,
/// and anchor for `input`/`output` paths.
///
/// When the ottofile path has a usable parent directory, that parent is the
/// base. Otherwise (no ottofile, or a bare filename whose `parent()` is `""`)
/// the caller-supplied `cwd` is returned.
///
/// This is the single source of truth for "where do relative paths in the
/// ottofile resolve from." It is the reason `otto deploy` works when invoked
/// from a subdirectory: discovery walks up to find the ottofile, then this
/// function pins all relative-path resolution to the ottofile's directory.
pub fn ottofile_base_dir<'a>(ottofile: Option<&'a Path>, cwd: &'a Path) -> &'a Path {
    ottofile
        .and_then(|p| p.parent())
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(cwd)
}

/// Check if a filename is a valid ottofile name.
/// This is a hidden/secret function used for shell scripting.
pub fn is_valid_ottofile_name(filename: &str) -> bool {
    OTTOFILES.contains(&filename)
}

/// Number of CPUs to default `--jobs` to. `available_parallelism()` returns an
/// `io::Result` because the count is genuinely unknowable on some platforms;
/// 1 is the same conservative fallback the crate this replaced used.
fn default_jobs() -> usize {
    std::thread::available_parallelism().map(NonZeroUsize::get).unwrap_or(1)
}

static DEFAULT_JOBS: LazyLock<String> = LazyLock::new(|| default_jobs().to_string());

/// Largest edit distance at which an unknown task name is worth suggesting a
/// replacement for. Beyond this the "did you mean" is noise, not help.
const MAX_SUGGESTION_DISTANCE: usize = 3;

/// Everything `otto` needs to execute a run, once the command line has been
/// parsed and the ottofile has been loaded.
#[derive(Debug)]
pub struct RunPlan {
    pub tasks: Vec<Task>,
    pub hash: String,
    pub ottofile: Option<PathBuf>,
    pub jobs: usize,
    pub tui_mode: bool,
    /// `--no-prefix`: suppress the `[task]` prefix on terminal output.
    pub no_prefix: bool,
}

impl RunPlan {
    /// The plan as a positional tuple, for call sites that only want one or two
    /// of the fields.
    #[allow(clippy::type_complexity)]
    pub fn into_parts(self) -> (Vec<Task>, String, Option<PathBuf>, usize, bool, bool) {
        (
            self.tasks,
            self.hash,
            self.ottofile,
            self.jobs,
            self.tui_mode,
            self.no_prefix,
        )
    }
}

/// What `Parser::parse` decided this invocation should do.
///
/// The parser is a library: it decides and reports, and `main` is the only
/// place allowed to end the process. `Exit` means the requested output (help,
/// version, `--tasks`, `--list-subtasks`) has already been written and the
/// process should terminate with the carried code, running no task.
#[derive(Debug)]
pub enum ParseOutcome {
    Run(RunPlan),
    Exit(i32),
}

impl ParseOutcome {
    /// The run plan, or an error if this invocation was not a run.
    pub fn into_run(self) -> Result<RunPlan> {
        match self {
            ParseOutcome::Run(plan) => Ok(plan),
            ParseOutcome::Exit(code) => Err(eyre!("expected a run plan, got an exit request with code {code}")),
        }
    }
}

/// The `-o/--ottofile` value this invocation asked for, without clap.
///
/// The help path cannot use clap's parsed value: clap errored out *with*
/// `DisplayHelp` before producing matches. It used to hardcode `"."` there, so
/// `otto -o sub/other.yml --help` and `OTTOFILE=sub/other.yml otto --help`
/// both listed the *divined* file's tasks while rendering
/// `[env: OTTOFILE=.../sub/other.yml]` in the same breath.
///
/// Precedence matches clap's: explicit flag, then env, then the default.
fn ottofile_value_from_args(args: &[String], env_value: Option<String>) -> OttofileSource {
    let mut i = 0;
    let mut found: Option<String> = None;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            break;
        }
        if arg == "-o" || arg == "--ottofile" {
            if let Some(value) = args.get(i + 1) {
                found = Some(value.clone());
            }
            i += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--ottofile=").or_else(|| arg.strip_prefix("-o=")) {
            found = Some(value.to_string());
        }
        i += 1;
    }

    found
        .or(env_value)
        .filter(|v| !v.is_empty())
        .map_or(OttofileSource::Divine, OttofileSource::Explicit)
}

/// The task name requested more than once on one command line, if any.
fn duplicate_task_name(partitions: &[Vec<String>]) -> Option<&str> {
    let mut seen: HashSet<&str> = HashSet::new();
    for partition in partitions {
        let Some(name) = partition.first() else {
            continue;
        };
        if !seen.insert(name.as_str()) {
            return Some(name.as_str());
        }
    }
    None
}

/// The error `otto build --msg=a build --msg=b` fails with.
///
/// It used to run `build` once and drop the second argument set on the floor -
/// a silent success with arguments the user watched themselves type.
fn duplicate_task_error(name: &str) -> eyre::Report {
    eyre!(
        "task '{name}' was requested more than once; otto runs each task once per invocation, \
         so the later arguments would be silently discarded"
    )
}

/// The declared choice matching `value`, ignoring case.
///
/// Choices are matched case-insensitively but stored canonically: the task's
/// `$param` env var must read `ascii` whether the user typed `ascii` or `ASCII`.
fn canonical_choice<'a>(value: &str, choices: &'a [String]) -> Option<&'a str> {
    choices
        .iter()
        .find(|choice| choice.eq_ignore_ascii_case(value))
        .map(String::as_str)
}

/// Strip a `--tui`/`-t` that landed in a task's arguments, reporting whether it
/// was there.
///
/// `--tui` is `.global(true)`, which clap does not propagate into external
/// subcommands - and every otto task is an external subcommand - so
/// `otto build --tui` failed with `unexpected argument '--tui'`. The declared
/// short (`-t`) was not stripped alongside it, so `otto build -t` failed the
/// same way against a flag `--help` says is global.
///
/// `take_short` is false when some task named in the same arg list declares its
/// own `-t`: then the token is that task's, not otto's, and eating it here
/// would make a declared param unreachable. Anything after a `--` belongs to
/// the task verbatim and is left alone.
fn take_tui_flag(args: Vec<String>, take_short: bool) -> (Vec<String>, bool) {
    let mut found = false;
    let mut kept = Vec::with_capacity(args.len());
    let mut iter = args.into_iter();
    for arg in iter.by_ref() {
        if arg == "--" {
            kept.push(arg);
            break;
        }
        if arg == "--tui" || (take_short && arg == "-t") {
            found = true;
            continue;
        }
        kept.push(arg);
    }
    kept.extend(iter);
    (kept, found)
}

/// Reject a task list that names both a builtin and an ordinary task.
///
/// A builtin is dispatched by `app::find_builtin`, which returns on the first
/// one it finds, so `otto build Clean` ran `Clean` and dropped `build` on the
/// floor with exit 0 - the user asked for two things, otto did one and said
/// nothing. The two cannot be combined (a builtin is not scheduled in the DAG),
/// so the honest answer is an error naming both sides.
fn reject_mixed_task_list(tasks_to_run: &[String]) -> Result<()> {
    let (builtins, others): (Vec<&str>, Vec<&str>) = tasks_to_run
        .iter()
        .map(String::as_str)
        .partition(|name| is_builtin(name));
    if builtins.is_empty() || others.is_empty() {
        return Ok(());
    }
    Err(eyre!(
        "cannot run builtin command(s) {} together with task(s) {}: \
         a builtin command runs on its own",
        quoted_list(&builtins),
        quoted_list(&others)
    ))
}

/// `a`, `b` -> `'a', 'b'`, for an error that has to name every offender.
fn quoted_list(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// True when `flag` appears in `args` as a flag, not as some option's value.
///
/// A raw `args.contains("--Serial")` matched `--msg --Serial` too, quietly
/// serializing a foreach group because a *value* spelled the flag.
fn contains_flag(args: &[String], flag: &str, value_options: Option<&HashSet<String>>) -> bool {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            return false;
        }
        if arg == flag {
            return true;
        }
        if value_options.is_some_and(|options| options.contains(arg.as_str())) {
            i += 2;
            continue;
        }
        i += 1;
    }
    false
}

/// A task's clap failure, with the `=` escape hatch named when it applies.
///
/// A partition that is followed by another one lost its tail to a task
/// boundary; if clap then rejects the value, the boundary is the likeliest
/// cause and the user needs to be told the way out.
fn task_arg_error(rendered: String, kind: clap::error::ErrorKind, next_task: Option<&str>) -> eyre::Report {
    match (kind, next_task) {
        (clap::error::ErrorKind::InvalidValue, Some(next)) => eyre!(
            "{rendered}\nhint: '{next}' was read as the next task to run; if it is meant as a value, \
             attach it to its flag with '=' (--flag={next})"
        ),
        _ => eyre!("{rendered}"),
    }
}

/// The leading args that `partitions()` would drop on the floor.
///
/// `partitions()` splits at the first arg that names a task, so everything
/// before that index is silently discarded. Those args are unknown task names
/// (a task's own flags always follow its name, never precede it), and this is
/// what makes them visible. Returns `None` when every arg is accounted for.
fn unconsumed_args<'a>(args: &'a [String], task_names: &[String]) -> Option<&'a [String]> {
    if args.is_empty() {
        return None;
    }
    let first_task = args.iter().position(|arg| task_names.contains(arg));
    match first_task {
        Some(0) => None,
        Some(i) => Some(&args[..i]),
        None => Some(args),
    }
}

/// The nearest known task name to `unknown`, if one is near enough to suggest.
fn nearest_task_name<'a>(unknown: &str, task_names: &'a [String]) -> Option<&'a str> {
    task_names
        .iter()
        // `help` is a partition-only entry, not a task to suggest. The
        // lowercase `"graph"` that used to be filtered here never appeared in
        // a production name list: the builtin is `Graph`.
        .filter(|name| name.as_str() != "help")
        .map(|name| (levenshtein::levenshtein(unknown, name), name.as_str()))
        .filter(|(distance, name)| *distance <= MAX_SUGGESTION_DISTANCE && *distance < name.len())
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|(_, name)| name)
}

/// The error `otto <unknown-task>` fails with, carrying a near-match
/// suggestion when there is one - the suggestion is what makes the new failure
/// self-explanatory instead of just louder.
fn unknown_task_error(unknown: &[String], task_names: &[String]) -> eyre::Report {
    let names: Vec<&str> = unknown.iter().map(String::as_str).collect();
    let head = if names.len() == 1 {
        format!("unknown task '{}'", names[0])
    } else {
        format!("unknown tasks: {}", names.join(", "))
    };
    match nearest_task_name(names[0], task_names) {
        Some(suggestion) => eyre!("{head}; did you mean '{suggestion}'?"),
        None => eyre!("{head}"),
    }
}

/// The error a bare `otto <task>` fails with when `task` declares a required
/// param and got no arguments at all - otto's own error, not clap's, because
/// clap never runs for this shape (`discovery.rs`'s bind gate skips it) and
/// the whole point of the preflight is not making it run just to say so.
fn required_param_error(task_name: &str, missing: &[&str]) -> eyre::Report {
    let names = missing.join(", ");
    eyre!("task '{task_name}': missing required param(s): {names}")
}

/// The clap `num_args` range implied by a param's `nargs:`.
///
/// `Nargs::Range(min, max)` stores the counts the user wrote: `nargs: "2:5"`
/// is `Range(2, 5)` and a bare `nargs: "3"` is `Range(3, 3)`, which clap
/// reads as exactly three values.
fn nargs_to_num_args(nargs: &Nargs) -> clap::builder::ValueRange {
    match nargs {
        Nargs::One => (1..=1).into(),
        Nargs::Zero => (0..=0).into(),
        Nargs::OneOrZero => (0..=1).into(),
        Nargs::OneOrMore => (1..).into(),
        Nargs::ZeroOrMore => (0..).into(),
        Nargs::Range(min, max) => (*min..=*max).into(),
    }
}

/// The first 8 hex digits of `content`'s sha256.
///
/// One function for both callers: a task's action hash and the ottofile's
/// project hash were the same four lines written twice, in two files.
fn short_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    let result = hasher.finalize();
    hex::encode(result)[..8].to_string()
}

fn ottofile_not_found_message() -> String {
    use colored::Colorize;

    let file_list = OTTOFILES
        .iter()
        .map(|f| format!("  {}", f.bright_yellow()))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "{}\n\nOtto looks for one of the following files:\n{}",
        "ERROR: No ottofile found in this directory or any parent directory!"
            .red()
            .bold(),
        file_list
    )
}

/// The truthful epilogue for the other config-failure state: the ottofile was
/// found and could not be parsed. Naming the file and the serde diagnostic
/// (field path, line, column) is the whole point - the not-found message would
/// be a lie here.
fn ottofile_parse_error_message(ottofile: &std::path::Path, err: &eyre::Report) -> String {
    use colored::Colorize;

    format!(
        "{} {}\n{}",
        "ERROR: failed to parse ottofile:".red().bold(),
        ottofile.display().to_string().bright_yellow(),
        err
    )
}

/// Serial foreach group membership: subtask name -> (group name, order index).
type SerialMembership = HashMap<String, (String, usize)>;

/// Buffered-foreach display order: parent task name -> subtask names in
/// declared foreach item order (design doc
/// `2026-08-31-buffered-foreach-computed-envs-required-params.md`, Phase 3).
/// Additive: built alongside `SerialMembership` at the same enumeration site
/// and read only by the Phase 4 replay cursor. Unlike the serial-ordering
/// fields above, this covers every foreach expansion, not just serial ones.
type DisplayOrderMap = HashMap<String, Vec<String>>;

/// Per-group concurrency: parent task name -> the permit count that group's
/// own semaphore is built with, already resolved against the expansion
/// (`jobs: all` is one permit per item, so the number is only knowable once
/// the items are known). Design doc
/// `2026-09-01-cancellation-reaping-and-foreach-concurrency.md`, Phase 3.
type ForeachJobsMap = HashMap<String, NonZeroUsize>;

/// Everything one pass of foreach expansion produces, so the five outputs stay
/// named rather than positional as the set grows (design doc Phase 4 added the
/// fourth, the foreach-concurrency doc the fifth). Only
/// `expand_foreach_tasks_with_serial` builds one.
struct ForeachExpansion {
    /// Every task in the run, with foreach tasks replaced by their subtasks
    /// plus a virtual parent.
    specs: TaskSpecs,
    /// Serial foreach group membership, for the scheduler's ready loop.
    membership: SerialMembership,
    /// Display order for every foreach expansion, buffered or not.
    display_order: DisplayOrderMap,
    /// Names of the foreach parents that declared `buffer: true`.
    buffered: HashSet<String>,
    /// Resolved per-group permit counts for the foreach parents that declared
    /// `jobs:`. Absent for every group that did not.
    jobs: ForeachJobsMap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub name: String,
    pub task_deps: Vec<TaskEdge>,
    pub file_deps: Vec<String>,
    pub output_deps: Vec<String>,
    pub envs: HashMap<String, String>,
    pub values: HashMap<String, Value>,
    pub action: String,
    pub hash: String,
    pub is_virtual_parent: bool,
    /// Name of the serial foreach group this task belongs to (the parent task name),
    /// or `None` for tasks that carry no ordering constraint. Serial ordering is a
    /// property of the task, not a dependency edge: it constrains start order without
    /// pulling predecessors into the run set.
    pub serial_group: Option<String>,
    /// Position of this task within `serial_group`, in declared foreach order.
    /// Meaningless when `serial_group` is `None`.
    pub serial_index: usize,
    /// This task owns the terminal: uncaptured, unprefixed, run exclusively.
    /// Carried from `TaskSpec::tty` so the `--tui` conflict is decidable before
    /// anything runs (`app::execute_tasks`).
    pub tty: bool,
    /// For a foreach virtual parent (buffered or not): its subtask names, in
    /// declared item order, mirroring each subtask's own `OTTO_FOREACH_INDEX`.
    /// `None` for every non-foreach task. Additive; read only by the Phase 4
    /// replay cursor, which needs it only when `buffer: true` (design doc,
    /// Phase 3).
    pub foreach_display_order: Option<Vec<String>>,
    /// True for a `foreach.buffer: true` parent and for every one of its
    /// subtasks. On a subtask it suppresses the live terminal leg; on the
    /// parent it marks the group the replay cursor owns (design doc
    /// `2026-08-31-buffered-foreach-computed-envs-required-params.md`,
    /// Phase 4). `buffer: true` itself does not survive expansion - subtasks
    /// are clones with `foreach = None` - so it is carried as this flag.
    pub buffered: bool,
    /// For an ITEM of a foreach group that declared `foreach.jobs`: the permit
    /// count that group's own semaphore is built with, one per item under
    /// `jobs: all`. `None` for every other task, including the group's virtual
    /// parent - the parent is queued only once its items are terminal, so it
    /// never runs beside them and has nothing to be exempted from (design doc
    /// `2026-09-01-cancellation-reaping-and-foreach-concurrency.md`, Phase 3).
    pub foreach_jobs: Option<NonZeroUsize>,
}

impl Task {
    #[must_use]
    pub fn new(
        name: String,
        task_deps: Vec<TaskEdge>,
        file_deps: Vec<String>,
        output_deps: Vec<String>,
        envs: HashMap<String, String>,
        values: HashMap<String, Value>,
        action: String,
    ) -> Self {
        let hash = short_hash(&action);
        Self {
            name,
            task_deps,
            file_deps,
            output_deps,
            envs,
            values,
            action,
            hash,
            is_virtual_parent: false,
            serial_group: None,
            serial_index: 0,
            tty: false,
            foreach_display_order: None,
            buffered: false,
            foreach_jobs: None,
        }
    }

    /// Fails closed on an unresolvable task environment.
    ///
    /// Dropping the map and running anyway was the silent-success this whole
    /// remediation exists to kill: one cyclic key took every *other* key with it,
    /// the task ran with an empty environment, and the run exited 0. The global
    /// env path next door (`cfg::resolver::global_envs`) already returns Err for
    /// the same failure; this is the same rule on the task path.
    pub fn from_task_with_cwd_and_global_envs(
        task_spec: &TaskSpec,
        cwd: &std::path::Path,
        global_envs: &HashMap<String, String>,
    ) -> Result<Self> {
        let name = task_spec.name.clone();
        let task_deps: Vec<TaskEdge> = task_spec
            .before
            .iter()
            .map(|e| TaskEdge::new(e.task.clone(), e.when))
            .collect();

        let evaluated_envs = Self::evaluate_merged_envs(global_envs, &task_spec.envs, cwd)
            .map_err(|e| eyre!("Failed to evaluate environment variables for task '{name}': {e}"))?;

        // Paths expand the task's evaluated environment before globbing: a
        // reference in an input/output path used to expand to nothing, so the
        // glob matched no file and the task silently never went up to date.
        let input_paths = crate::cfg::task::expand_env_in_paths(&name, "input", &task_spec.input, &evaluated_envs)?;
        let output_paths = crate::cfg::task::expand_env_in_paths(&name, "output", &task_spec.output, &evaluated_envs)?;
        let file_deps = Self::resolve_file_globs(&input_paths, cwd);
        let output_deps = Self::resolve_file_globs(&output_paths, cwd);

        // Note: We do NOT add after tasks here since they depend on us, not vice versa
        // The after dependencies will be handled during DAG construction
        let values = HashMap::new();
        let action = task_spec.action.trim().to_string(); // Trim whitespace from script content
        let mut t = Self::new(name, task_deps, file_deps, output_deps, evaluated_envs, values, action);
        t.is_virtual_parent = task_spec.virtual_parent;
        t.tty = task_spec.tty.unwrap_or(false);
        Ok(t)
    }

    /// Evaluate and merge environment variables from global and task-level sources
    fn evaluate_merged_envs(
        global_envs: &HashMap<String, String>,
        task_envs: &HashMap<String, String>,
        cwd: &std::path::Path,
    ) -> Result<HashMap<String, String>> {
        let mut merged_envs = HashMap::new();

        for (key, value) in global_envs {
            merged_envs.insert(key.clone(), value.clone());
        }

        // Then, evaluate and add task-level environment variables (overriding global ones)
        if !task_envs.is_empty() {
            let task_evaluated_envs = env_eval::evaluate_envs(task_envs, Some(cwd), &HashMap::new())?;
            for (key, value) in task_evaluated_envs {
                merged_envs.insert(key, value);
            }
        }

        Ok(merged_envs)
    }

    /// Resolve file globs to canonical paths
    fn resolve_file_globs(patterns: &[String], cwd: &std::path::Path) -> Vec<String> {
        let mut resolved_paths = Vec::new();

        for pattern in patterns {
            // Use glob to expand the pattern
            let full_pattern = if std::path::Path::new(pattern).is_absolute() {
                pattern.clone()
            } else {
                cwd.join(pattern).to_string_lossy().to_string()
            };

            match glob::glob(&full_pattern) {
                Ok(paths) => {
                    for path in paths {
                        match path {
                            Ok(p) => {
                                // Convert to canonical path
                                match fs::canonicalize(&p) {
                                    Ok(canonical) => resolved_paths.push(canonical.to_string_lossy().to_string()),
                                    Err(_) => {
                                        // If canonicalization fails, use the original path
                                        resolved_paths.push(p.to_string_lossy().to_string());
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("Warning: Failed to resolve glob pattern '{pattern}': {e}");
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Warning: Invalid glob pattern '{pattern}': {e}");
                    // If glob fails, treat as literal path
                    resolved_paths.push(pattern.clone());
                }
            }
        }

        resolved_paths
    }
}

/// Why a task's clap `Command` is being built.
///
/// Dynamic (command-sourced) config wants opposite things from the two callers,
/// so the caller states which it is rather than leaving it to be inferred from
/// an unrelated argument's `Option`-ness:
///
/// - `Help` renders. It executes nothing, ever - a dynamic source is described
///   (`[dynamic]`, `[dynamic choices: <cmd>]`), never run.
/// - `Bind` validates real CLI arguments. It resolves dynamic sources through
///   the per-invocation cache, and can therefore fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildMode {
    Help,
    Bind,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Parser {
    cwd: PathBuf,
    config_spec: ConfigSpec,
    hash: String,
    args: Vec<String>,
    pargs: Vec<Vec<String>>,
    ottofile: Option<PathBuf>,
    jobs: usize,
    /// Per-invocation cache for dynamic (command-sourced) config values, plus
    /// the memoized global `envs:` those commands run with. Interior-mutable so
    /// the `&self` call sites (partitioning, `--list-subtasks`, `--tasks`,
    /// expansion) all share one resolution. See src/cfg/resolver.rs.
    resolver: DynamicResolver,
}

impl Parser {
    pub fn new(args: Vec<String>) -> Result<Self> {
        let cwd = crate::executor::workspace::current_dir()?;

        Ok(Self {
            cwd,
            config_spec: ConfigSpec::default(),
            hash: String::new(),
            args,
            pargs: Vec::new(),
            ottofile: None,
            jobs: default_jobs(),
            resolver: DynamicResolver::new(),
        })
    }

    /// Returns the base directory for resolving relative paths in the ottofile.
    /// Thin wrapper over the free `ottofile_base_dir` so parser internals and
    /// task execution share one definition of "workspace root."
    fn base_dir(&self) -> &Path {
        ottofile_base_dir(self.ottofile.as_deref(), &self.cwd)
    }

    /// The resolved global `envs:`, evaluated at most once per invocation.
    ///
    /// This is the environment contract for command-sourced foreach: the
    /// command sees the inherited environment plus these. Memoizing here (and
    /// having `process_tasks_with_filter` read the same accessor) is what puts
    /// global env evaluation ahead of arg partitioning: whichever site needs a
    /// command source first, the envs are resolved before the command runs.
    /// Global envs do not depend on task args, so the values are identical
    /// wherever the first ask happens.
    ///
    /// A failure here is fatal, not a warning: dropping the globals and carrying
    /// on ran the tasks with an environment nobody configured (a >100-deep or
    /// circular chain hit exactly this and the run continued, silently
    /// env-less).
    ///
    /// The cwd is `base_dir()`, the ottofile's own directory, matching
    /// `foreach.command` (`:679`) and `choices-command`
    /// (`parser/command.rs:165`). It used to be the *process* cwd, so
    /// `envs: {ROOT: '$(scripts/svc.sh root web)'}` resolved relative paths
    /// against whoever ran otto: verified failing with exit 127 on plain
    /// `otto <task>` from a subdirectory, on `-C` aimed anywhere but the
    /// ottofile's directory, on `-o`, and on `$OTTOFILE`. One cwd contract for
    /// all four command sources (design doc
    /// `docs/design/2026-08-31-buffered-foreach-computed-envs-required-params.md`,
    /// Phase 2).
    fn global_envs(&self) -> Result<&HashMap<String, String>> {
        self.resolver.global_envs(|| {
            // Inside the initializer, ahead of `evaluate_envs`, and NOT through
            // `DynamicResolver`'s own caches: those sit downstream of this
            // `OnceCell`, and re-entering it from its own initializer panics.
            // The cell already gives once-per-invocation, and being here is
            // what makes `envs-command` inherit `envs:`' laziness exactly -
            // never for `--help`, otherwise whenever something needs the map.
            let computed = self.resolve_envs_command()?;
            if self.config_spec.otto.envs.is_empty() && computed.is_empty() {
                return Ok(HashMap::new());
            }
            env_eval::evaluate_envs(&self.config_spec.otto.envs, Some(self.base_dir()), &computed)
        })
    }

    /// Run `otto.envs-command` and parse its `KEY=VALUE` stdout.
    ///
    /// Shares `run_command_stdout`'s execution contract (guard chain, `sh -c`,
    /// cwd = `base_dir()`, loud non-zero exit, stderr passthrough) with the raw
    /// stdout form, so whitespace inside a value survives byte-for-byte where
    /// `run_lines_command`'s per-line `trim` would have eaten it.
    ///
    /// Two things the shared contract cannot give it: the guard key is a fixed
    /// literal rather than a task name (one resolution per invocation), and the
    /// command's environment is the inherited one only, because the global env
    /// map is what this call is in the middle of computing.
    fn resolve_envs_command(&self) -> Result<HashMap<String, String>> {
        let Some(command) = self.config_spec.otto.envs_command.as_deref() else {
            return Ok(HashMap::new());
        };
        let stdout = resolver::run_command_stdout(
            command,
            self.base_dir(),
            &HashMap::new(),
            resolver::ENVS_GUARD_VAR,
            ENVS_GUARD_KEY,
            ENVS_COMMAND_CONTEXT,
        )?;
        env_eval::parse_env_assignments(&stdout)
            .map_err(|e| eyre!("{ENVS_COMMAND_CONTEXT}: command '{command}' produced invalid output: {e}"))
    }

    /// Resolve one task's foreach items, running a command source at most once
    /// per invocation. Static sources (glob / items / range) take exactly the
    /// path they always have.
    fn resolve_foreach(&self, task_name: &str, foreach: &ForeachSpec) -> Result<Vec<ForeachItem>> {
        if !foreach.is_command_source() {
            return foreach.resolve_items(self.base_dir());
        }
        self.resolver.foreach_items(task_name, || {
            foreach.resolve_command_items(task_name, self.base_dir(), self.global_envs()?)
        })
    }

    /// True when the requested args name `task` or one of its subtasks
    /// (`up:gamma`). This is the lazy trigger for a command source: no mention,
    /// no execution.
    fn args_mention_task(args: &[String], task: &str) -> bool {
        let prefix = format!("{task}:");
        args.iter().any(|arg| arg == task || arg.starts_with(&prefix))
    }

    /// Returns a reference to the original task specs from the ottofile.
    ///
    /// This provides access to the raw TaskSpecs including ForeachSpec metadata,
    /// before foreach expansion. Useful for graph visualization to determine
    /// how to display collapsed subtask groups (e.g., showing glob pattern vs items list).
    pub fn original_task_specs(&self) -> &TaskSpecs {
        &self.config_spec.tasks
    }

    /// Returns the retention configuration from the ottofile.
    pub fn retention(&self) -> crate::cfg::otto::RetentionSpec {
        self.config_spec.otto.retention.clone()
    }

    /// Parse the command line and decide what this invocation should do.
    ///
    /// Never terminates the process: every path that used to end the process
    /// from library code now returns `ParseOutcome::Exit(code)` after writing
    /// its own output, so the only place that ends the process is `main`.
    pub fn parse(&mut self) -> Result<ParseOutcome> {
        log::debug!("parse: args={:?}", self.args);
        let help_requested = self.args.contains(&"--help".to_string()) || self.args.contains(&"-h".to_string());

        let otto_cmd = Self::otto_command();
        let matches = match otto_cmd.try_get_matches_from(&self.args) {
            Ok(m) => m,
            Err(e) => {
                use clap::error::ErrorKind;
                match e.kind() {
                    ErrorKind::DisplayVersion => {
                        e.print().expect("clap error print failed");
                        return Ok(ParseOutcome::Exit(0));
                    }
                    ErrorKind::DisplayHelp => {
                        if help_requested {
                            // Read the flag and the env var ourselves: clap
                            // bailed with DisplayHelp before producing matches,
                            // and hardcoding "." here is what made
                            // `otto -o other.yml --help` list the wrong file's
                            // tasks while printing other.yml's path in the same
                            // output.
                            let ottofile_value = ottofile_value_from_args(&self.args, env::var("OTTOFILE").ok());
                            let ottofile_path = Self::divine_ottofile(ottofile_value);

                            match ottofile_path {
                                Ok(Some(path)) => {
                                    // Ottofile exists, load config and show normal help with tasks
                                    match Self::load_config_from_path(Some(path.clone())) {
                                        Ok((config_spec, _, _)) => {
                                            let mut temp_parser = Self {
                                                cwd: self.cwd.clone(),
                                                config_spec,
                                                hash: String::new(),
                                                args: self.args.clone(),
                                                pargs: Vec::new(),
                                                // The real path, not `None`:
                                                // `base_dir()` reads this, so
                                                // help used to resolve foreach
                                                // globs against the cwd instead
                                                // of the ottofile's directory
                                                // and rendered `[0 items]` from
                                                // a subdirectory.
                                                ottofile: Some(path.clone()),
                                                jobs: default_jobs(),
                                                resolver: DynamicResolver::new(),
                                            };
                                            temp_parser.inject_builtin_commands();
                                            let mut help_cmd = temp_parser.build_help_command();
                                            help_cmd.print_help().expect("Failed to print help");
                                            return Ok(ParseOutcome::Exit(0));
                                        }
                                        Err(e) => {
                                            // The ottofile EXISTS and failed to
                                            // parse. Render the global flags
                                            // without the not-found epilogue -
                                            // claiming the file is missing when
                                            // it is merely malformed sends the
                                            // operator hunting for the wrong
                                            // problem - and put the real serde
                                            // diagnostic on stderr, the same
                                            // stream `--tasks` reports on.
                                            let mut help_cmd = Self::build_bare_help_command();
                                            help_cmd.print_help().expect("Failed to print help");
                                            eprintln!("{}", ottofile_parse_error_message(&path, &e));
                                            return Ok(ParseOutcome::Exit(2));
                                        }
                                    }
                                }
                                _ => {
                                    // No ottofile found, show help with error message
                                    let mut help_cmd = Self::build_help_command_with_error();
                                    help_cmd.print_help().expect("Failed to print help");
                                    return Ok(ParseOutcome::Exit(2));
                                }
                            }
                        } else {
                            e.print().expect("clap error print failed");
                            return Ok(ParseOutcome::Exit(0));
                        }
                    }
                    _ => return Err(eyre!(e)),
                }
            }
        };

        // Extract ottofile and load config. Absent means Divine; see
        // `OttofileSource`.
        let ottofile_value = matches
            .get_one::<String>("ottofile")
            .cloned()
            .map_or(OttofileSource::Divine, OttofileSource::Explicit);

        // Extract jobs parameter. The value parser rejects 0 and non-numbers at
        // parse time (clap exits 2 with a usage error), so anything that reaches
        // here is a usable concurrency limit - `-j 0` used to be accepted and
        // then hot-spin the launch loop at 100% CPU forever.
        self.jobs = usize::try_from(*matches.get_one::<u64>("jobs").expect("jobs should have default value"))
            .map_err(|_| eyre!("-j/--jobs value is too large for this platform"))?;
        // `-j/--jobs` always has a value (clap's default is the CPU count), so
        // the only way to tell "the user actually typed -j" from "clap filled
        // it in" is `value_source`. An ottofile's `otto.jobs` gets to set the
        // default only when the flag was not given explicitly; either way the
        // flag on the command line always wins.
        let jobs_explicit = !matches!(
            matches.value_source("jobs"),
            Some(clap::parser::ValueSource::DefaultValue)
        );

        // Extract tui flag
        let mut tui_mode = matches.get_flag("tui");

        // Extract no-prefix flag (see docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md Phase 8)
        let no_prefix = matches.get_flag("no-prefix");

        let ottofile_path = Self::divine_ottofile(ottofile_value)?;
        let (config_spec, hash, ottofile) = Self::load_config_from_path(ottofile_path)?;

        self.config_spec = config_spec;
        self.hash = hash;
        self.ottofile = ottofile;

        // `otto.jobs` is an `Option`: absent leaves the CPU-count default that
        // clap already filled in, present overrides it.
        if !jobs_explicit && let Some(jobs) = self.config_spec.otto.jobs {
            self.jobs = jobs;
        }

        // Inject built-in commands
        self.inject_builtin_commands();

        // Handle --list-subtasks flag
        if matches.get_flag("list-subtasks") {
            if let Err(e) = self.print_subtasks() {
                // `{e:#}` prints the whole cause chain: a wrapped foreach
                // failure must name the command and its exit code, not just
                // the outermost "failed to resolve" context.
                eprintln!("Error: {e:#}");
                return Ok(ParseOutcome::Exit(1));
            }
            return Ok(ParseOutcome::Exit(0));
        }

        // Handle --tasks flag: emit the machine-readable task list and exit,
        // executing no task. See docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md
        // Phase 5 for the frozen contract (no builtins, subtasks nested once,
        // stdout is pure data, notices/errors on stderr).
        if matches.get_flag("tasks") {
            match crate::cli::commands::tasks::build_tasks_view(&self.config_spec.tasks, &|name, foreach| {
                self.resolve_foreach(name, foreach)
            }) {
                Ok(view) => {
                    // `ignore_case(true)` hands back what the user typed, so
                    // canonicalize before the format table looks it up.
                    let explicit_format = matches.get_one::<String>("format").map(|f| f.to_ascii_lowercase());
                    let explicit_format = explicit_format.as_deref();
                    let stdout_is_tty = std::io::stdout().is_terminal();
                    let format = crate::cli::commands::tasks::choose_format(explicit_format, stdout_is_tty);
                    match crate::cli::commands::tasks::render_tasks_view(&view, format) {
                        Ok(rendered) => {
                            println!("{rendered}");
                            return Ok(ParseOutcome::Exit(0));
                        }
                        Err(e) => {
                            eprintln!("Error: {e:#}");
                            return Ok(ParseOutcome::Exit(1));
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e:#}");
                    return Ok(ParseOutcome::Exit(1));
                }
            }
        }

        // Extract remaining arguments after global options
        let remaining_args = self.extract_remaining_args(&matches);

        // `--tui` is global but clap does not push global flags into external
        // subcommands, so `otto build --tui` arrives here as a task argument.
        let take_short_t = !self.args_claim_short(&remaining_args, 't');
        let (remaining_args, tui_in_task_args) = take_tui_flag(remaining_args, take_short_t);
        tui_mode = tui_mode || tui_in_task_args;

        // Handle help commands
        if self.should_show_help(&remaining_args) {
            self.show_help(&remaining_args)?;
            return Ok(ParseOutcome::Exit(0));
        }

        // SECOND PASS: Determine which tasks to run
        let tasks_to_run = if remaining_args.is_empty() {
            // No task arguments provided - use default tasks from config
            self.resolve_default_tasks()?
        } else {
            // Task arguments provided - partition and parse them
            let task_names = self.get_task_names(&remaining_args)?;
            // An arg that no partition consumes is a task name otto does not
            // know. Erroring here is what stops `otto nonexistent` from being a
            // silent exit 0 and `otto nonexistent build` from quietly running
            // build.
            if let Some(unknown) = unconsumed_args(&remaining_args, &task_names) {
                return Err(unknown_task_error(unknown, &task_names));
            }
            let value_options = self.value_taking_options(&task_names);
            let partitions = partitions(&remaining_args, &task_names, &value_options);
            if let Some(duplicate) = duplicate_task_name(&partitions) {
                return Err(duplicate_task_error(duplicate));
            }
            self.pargs = partitions;

            // Extract task names from partitions
            self.extract_task_names_from_partitions()
        };

        // A builtin runs alone; naming one alongside an ordinary task used to
        // run the builtin and silently drop the rest.
        reject_mixed_task_list(&tasks_to_run)?;

        // Process tasks and build DAG
        let tasks = self.process_tasks_with_filter(&tasks_to_run)?;

        Ok(ParseOutcome::Run(RunPlan {
            tasks,
            hash: self.hash.clone(),
            ottofile: self.ottofile.clone(),
            jobs: self.jobs,
            tui_mode,
            no_prefix,
        }))
    }

    pub fn parse_all_tasks(&mut self) -> Result<(Vec<Task>, String, Option<PathBuf>)> {
        log::debug!("parse_all_tasks: args={:?}", self.args);
        // Load config if not already loaded
        if self.config_spec.tasks.is_empty() {
            // Parse command line arguments to extract ottofile path (similar to main parse method)
            let otto_cmd = Self::otto_command();

            // Both branches want one thing - where to look for the ottofile -
            // and then load it identically, so only that differs here. They
            // used to be two copies of the whole load-and-process tail.
            let ottofile_value = match otto_cmd.try_get_matches_from(&self.args) {
                // Clap handles the env var. No clap default: absent means
                // Divine, which is a state and not a path. `expect`ing a value
                // here is what the `"."` sentinel existed to satisfy.
                Ok(matches) => matches
                    .get_one::<String>("ottofile")
                    .cloned()
                    .map_or(OttofileSource::Divine, OttofileSource::Explicit),
                // Parsing failed, so read the flag and env directly rather
                // than divining from the cwd and ignoring what was asked for.
                Err(_) => ottofile_value_from_args(&self.args, env::var("OTTOFILE").ok()),
            };

            let ottofile_path = Self::divine_ottofile(ottofile_value)?;
            let (config_spec, hash, ottofile) = Self::load_config_from_path(ottofile_path)?;

            self.config_spec = config_spec;
            self.hash = hash;
            self.ottofile = ottofile;
        }

        let all_task_names: Vec<String> = self
            .config_spec
            .tasks
            .keys()
            .filter(|name| !is_builtin(name))
            .cloned()
            .collect();

        // Process all tasks
        let tasks = self.process_tasks_with_filter(&all_task_names)?;

        Ok((tasks, self.hash.clone(), self.ottofile.clone()))
    }
}

// The rest of `impl Parser` is split across sibling files to keep this file
// under the 1500-line cap (Phase 9). Each `include!`d file is a self-contained
// `impl Parser { ... }` block for the same type in the same module, so
// visibility and method resolution behave exactly as if it were all one file.
include!("parser/help.rs");
include!("parser/discovery.rs");
include!("parser/params.rs");
include!("parser/foreach.rs");
include!("parser/command.rs");
include!("parser/meta_tasks.rs");
include!("parser/config.rs");

/// Which option tokens consume the next argument as a value, per task.
///
/// `task name -> {"--msg", "-m", ...}`. Built from the task specs so
/// partitioning can tell a task name from a value that happens to spell one.
pub type ValueTakingOptions = HashMap<String, HashSet<String>>;

/// The argument indices that start a new task's partition.
///
/// Two things stop an argument from being a boundary:
///
/// - it is the value of the preceding option (`otto build --msg test` split at
///   `test` and left `--msg` with nothing, so clap reported "a value is
///   required for '--msg <msg>'" for an argument the user did supply);
/// - it follows a `--`, which hands the rest of the line to the current task
///   verbatim.
///
/// Only the single token after an option is protected. A multi-value option
/// (`nargs: "2:"`) whose *second* value spells a task name still splits; use
/// `--flag=value` there.
fn indices(args: &[String], task_names: &[String], value_options: &ValueTakingOptions) -> Vec<usize> {
    let mut indices = vec![];
    let mut current: Option<&str> = None;
    let mut verbatim = false;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];

        if arg == "--" && current.is_some() {
            verbatim = true;
            i += 1;
            continue;
        }

        if !verbatim && task_names.contains(arg) {
            indices.push(i);
            current = Some(arg.as_str());
            i += 1;
            continue;
        }

        if !verbatim
            && let Some(task) = current
            && let Some(options) = value_options.get(task)
            && options.contains(arg.as_str())
        {
            // The next token is this option's value, whatever it spells.
            i += 2;
            continue;
        }

        i += 1;
    }

    indices
}

fn partitions(args: &[String], task_names: &[String], value_options: &ValueTakingOptions) -> Vec<Vec<String>> {
    let task_indices = indices(args, task_names, value_options);
    if task_indices.is_empty() {
        return vec![];
    }

    let mut partitions: Vec<Vec<String>> = vec![];
    let mut end = args.len();

    for &index in task_indices.iter().rev() {
        partitions.insert(0, args[index..end].to_vec());
        end = index;
    }

    // The otto-level `--` has done its job (it stopped the split); the task's
    // own clap must not see it, or everything after it becomes a positional.
    for partition in &mut partitions {
        if let Some(pos) = partition.iter().position(|a| a == "--") {
            partition.remove(pos);
        }
    }

    partitions
}

#[path = "parser_tests_a.rs"]
mod tests_a;
#[path = "parser_tests_b.rs"]
mod tests_b;
