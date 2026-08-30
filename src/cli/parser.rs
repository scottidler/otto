//#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::fmt::Debug;
use std::fs;
use std::path::{Path, PathBuf};

use crate::executor::task::TaskEdge;

use clap::{Arg, ArgMatches, Command, value_parser};
use daggy::Dag;
use expanduser::expanduser;
use eyre::{Result, eyre};
use glob;
use hex;
use once_cell::sync::Lazy;
use sha2::{Digest, Sha256};

use crate::cfg::config::{ConfigSpec, ParamSpec, TaskSpec, Value};
use crate::cfg::env as env_eval;
use crate::cfg::param::ParamType;
use crate::cfg::resolver::DynamicResolver;
use crate::cfg::task::{ForeachItem, ForeachSpec, TaskSpecs};
use crate::cli::builtins::BUILTIN_COMMANDS;

pub type DAG<T> = Dag<T, (), u32>;

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

static DEFAULT_JOBS: Lazy<String> = Lazy::new(|| num_cpus::get().to_string());

fn calculate_hash(action: &String) -> String {
    let mut hasher = Sha256::new();
    hasher.update(action);
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

#[derive(Debug)]
pub struct OttofileNotFound;

impl std::fmt::Display for OttofileNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", ottofile_not_found_message())
    }
}

impl std::error::Error for OttofileNotFound {}

/// Serial foreach group membership: subtask name -> (group name, order index).
type SerialMembership = HashMap<String, (String, usize)>;

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
        let hash = calculate_hash(&action);
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
        }
    }

    #[must_use]
    pub fn from_task_with_cwd_and_global_envs(
        task_spec: &TaskSpec,
        cwd: &std::path::Path,
        global_envs: &HashMap<String, String>,
    ) -> Self {
        let name = task_spec.name.clone();
        let task_deps: Vec<TaskEdge> = task_spec
            .before
            .iter()
            .map(|e| TaskEdge::new(e.task.clone(), e.when))
            .collect();

        // Resolve file globs from input to canonical paths using explicit cwd
        let file_deps = Self::resolve_file_globs(&task_spec.input, cwd);

        // Resolve output globs to canonical paths using explicit cwd
        let output_deps = Self::resolve_file_globs(&task_spec.output, cwd);

        let evaluated_envs = Self::evaluate_merged_envs(global_envs, &task_spec.envs, cwd).unwrap_or_else(|e| {
            eprintln!("Warning: Failed to evaluate environment variables for task '{name}': {e}");
            HashMap::new()
        });

        // Note: We do NOT add after tasks here since they depend on us, not vice versa
        // The after dependencies will be handled during DAG construction
        let values = HashMap::new();
        let action = task_spec.action.trim().to_string(); // Trim whitespace from script content
        let mut t = Self::new(name, task_deps, file_deps, output_deps, evaluated_envs, values, action);
        t.is_virtual_parent = task_spec.virtual_parent;
        t.tty = task_spec.tty.unwrap_or(false);
        t
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
            let task_evaluated_envs = env_eval::evaluate_envs(task_envs, Some(cwd))?;
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
    prog: String,
    cwd: PathBuf,
    user: String,
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
        let prog = args.first().cloned().unwrap_or_else(|| "otto".to_string());
        let cwd = env::current_dir()?;
        let user = env::var("USER").unwrap_or_else(|_| "unknown".to_string());

        Ok(Self {
            prog,
            cwd,
            user,
            config_spec: ConfigSpec::default(),
            hash: String::new(),
            args,
            pargs: Vec::new(),
            ottofile: None,
            jobs: num_cpus::get(), // Default to number of CPUs
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
    fn global_envs(&self) -> &HashMap<String, String> {
        self.resolver.global_envs(|| {
            if self.config_spec.otto.envs.is_empty() {
                HashMap::new()
            } else {
                env_eval::evaluate_envs(&self.config_spec.otto.envs, Some(&self.cwd)).unwrap_or_else(|e| {
                    eprintln!("Warning: Failed to evaluate global environment variables: {e}");
                    HashMap::new()
                })
            }
        })
    }

    /// Resolve one task's foreach items, running a command source at most once
    /// per invocation. Static sources (glob / items / range) take exactly the
    /// path they always have.
    fn resolve_foreach(&self, task_name: &str, foreach: &ForeachSpec) -> Result<Vec<ForeachItem>> {
        if !foreach.is_command_source() {
            return foreach.resolve_items(self.base_dir());
        }
        self.resolver.foreach_items(task_name, || {
            foreach.resolve_command_items(task_name, self.base_dir(), self.global_envs())
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

    #[allow(clippy::type_complexity)]
    pub fn parse(&mut self) -> Result<(Vec<Task>, String, Option<PathBuf>, usize, bool, bool)> {
        let help_requested = self.args.contains(&"--help".to_string()) || self.args.contains(&"-h".to_string());

        let otto_cmd = Self::otto_command();
        let matches = match otto_cmd.try_get_matches_from(&self.args) {
            Ok(m) => m,
            Err(e) => {
                use clap::error::ErrorKind;
                match e.kind() {
                    ErrorKind::DisplayVersion => {
                        e.print().expect("clap error print failed");
                        std::process::exit(0);
                    }
                    ErrorKind::DisplayHelp => {
                        if help_requested {
                            let ottofile_value = ".".to_string();
                            let ottofile_path = Self::divine_ottofile(ottofile_value);

                            match ottofile_path {
                                Ok(Some(path)) => {
                                    // Ottofile exists, load config and show normal help with tasks
                                    match Self::load_config_from_path(Some(path.clone())) {
                                        Ok((config_spec, _, _)) => {
                                            let mut temp_parser = Self {
                                                prog: self.prog.clone(),
                                                cwd: self.cwd.clone(),
                                                user: self.user.clone(),
                                                config_spec,
                                                hash: String::new(),
                                                args: self.args.clone(),
                                                pargs: Vec::new(),
                                                ottofile: None,
                                                jobs: num_cpus::get(),
                                                resolver: DynamicResolver::new(),
                                            };
                                            temp_parser.inject_builtin_commands();
                                            let mut help_cmd = temp_parser.build_help_command();
                                            help_cmd.print_help().expect("Failed to print help");
                                            std::process::exit(0);
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
                                            std::process::exit(2);
                                        }
                                    }
                                }
                                _ => {
                                    // No ottofile found, show help with error message
                                    let mut help_cmd = Self::build_help_command_with_error();
                                    help_cmd.print_help().expect("Failed to print help");
                                    std::process::exit(2);
                                }
                            }
                        } else {
                            e.print().expect("clap error print failed");
                            std::process::exit(0);
                        }
                    }
                    _ => return Err(eyre!(e)),
                }
            }
        };

        // Extract ottofile and load config
        let ottofile_value = matches
            .get_one::<String>("ottofile")
            .cloned()
            .expect("ottofile should have a value from flag, env var, or default");

        // Extract jobs parameter (has default value from DEFAULT_JOBS)
        let jobs_str = matches
            .get_one::<String>("jobs")
            .expect("jobs should have default value");
        self.jobs = jobs_str.parse::<usize>().unwrap_or_else(|_| {
            eprintln!(
                "Warning: Invalid jobs value '{}', using {} CPUs",
                jobs_str,
                num_cpus::get()
            );
            num_cpus::get()
        });

        // Extract tui flag
        let tui_mode = matches.get_flag("tui");

        // Extract no-prefix flag (see docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md Phase 8)
        let no_prefix = matches.get_flag("no-prefix");

        let ottofile_path = Self::divine_ottofile(ottofile_value)?;
        let (config_spec, hash, ottofile) = Self::load_config_from_path(ottofile_path)?;

        self.config_spec = config_spec;
        self.hash = hash;
        self.ottofile = ottofile;

        // Inject built-in commands
        self.inject_builtin_commands();

        // Handle --list-subtasks flag
        if matches.get_flag("list-subtasks") {
            if let Err(e) = self.print_subtasks() {
                // `{e:#}` prints the whole cause chain: a wrapped foreach
                // failure must name the command and its exit code, not just
                // the outermost "failed to resolve" context.
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
            std::process::exit(0);
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
                    let explicit_format = matches.get_one::<String>("format").map(String::as_str);
                    let stdout_is_tty = atty::is(atty::Stream::Stdout);
                    let format = crate::cli::commands::tasks::choose_format(explicit_format, stdout_is_tty);
                    match crate::cli::commands::tasks::render_tasks_view(&view, format) {
                        Ok(rendered) => {
                            println!("{rendered}");
                            std::process::exit(0);
                        }
                        Err(e) => {
                            eprintln!("Error: {e:#}");
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e:#}");
                    std::process::exit(1);
                }
            }
        }

        // Extract remaining arguments after global options
        let remaining_args = self.extract_remaining_args(&matches);

        // Handle help commands
        if self.should_show_help(&remaining_args) {
            self.show_help(&remaining_args)?;
            std::process::exit(0);
        }

        // SECOND PASS: Determine which tasks to run
        let tasks_to_run = if remaining_args.is_empty() {
            // No task arguments provided - use default tasks from config
            self.resolve_default_tasks()?
        } else {
            // Task arguments provided - partition and parse them
            let task_names = self.get_task_names(&remaining_args)?;
            let partitions = partitions(&remaining_args, &task_names);
            self.pargs = partitions;

            // Extract task names from partitions
            self.extract_task_names_from_partitions()
        };

        // Process tasks and build DAG
        let tasks = self.process_tasks_with_filter(&tasks_to_run)?;

        Ok((
            tasks,
            self.hash.clone(),
            self.ottofile.clone(),
            self.jobs,
            tui_mode,
            no_prefix,
        ))
    }

    pub fn parse_all_tasks(&mut self) -> Result<(Vec<Task>, String, Option<PathBuf>)> {
        // Load config if not already loaded
        if self.config_spec.tasks.is_empty() {
            // Parse command line arguments to extract ottofile path (similar to main parse method)
            let otto_cmd = Self::otto_command();

            // Try to parse with allow_external_subcommands to capture ottofile flag
            let matches = match otto_cmd.try_get_matches_from(&self.args) {
                Ok(m) => m,
                Err(_) => {
                    // If parsing fails, fall back to default value
                    let ottofile_value = "./".to_owned();
                    let ottofile_path = Self::divine_ottofile(ottofile_value)?;
                    let (config_spec, hash, ottofile) = Self::load_config_from_path(ottofile_path)?;

                    self.config_spec = config_spec;
                    self.hash = hash;
                    self.ottofile = ottofile;

                    let all_task_names: Vec<String> = self
                        .config_spec
                        .tasks
                        .keys()
                        .filter(|name| *name != "graph")
                        .cloned()
                        .collect();

                    // Process all tasks
                    let tasks = self.process_tasks_with_filter(&all_task_names)?;

                    return Ok((tasks, self.hash.clone(), self.ottofile.clone()));
                }
            };

            // Extract ottofile path from parsed arguments (Clap handles env var automatically)
            let ottofile_value = matches
                .get_one::<String>("ottofile")
                .cloned()
                .expect("ottofile should have a value from flag, env var, or default");

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
            .filter(|name| *name != "graph")
            .cloned()
            .collect();

        // Process all tasks
        let tasks = self.process_tasks_with_filter(&all_task_names)?;

        Ok((tasks, self.hash.clone(), self.ottofile.clone()))
    }

    /// Single source of truth for otto's global flags: cwd, ottofile,
    /// list-subtasks, jobs, tui, no-prefix. `otto_command()`, `build_help_command()`,
    /// and `build_help_command_with_error()` all consume this so the
    /// rendered `--help` output can never drift from what's actually parsed
    /// again (see docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md
    /// Phase 1 - the prior drift: the two help builders re-declared only
    /// `jobs` and `tui`, silently omitting `-C/--cwd`, `-o/--ottofile`, and
    /// `--list-subtasks` from `otto --help`).
    fn global_args() -> Vec<Arg> {
        vec![
            Arg::new("cwd")
                .short('C')
                .long("cwd")
                .value_name("DIR")
                .help("Change to DIR before doing anything")
                .value_parser(value_parser!(String)),
            Arg::new("ottofile")
                .short('o')
                .long("ottofile")
                .value_name("PATH")
                .help("path to the ottofile")
                .default_value(".")
                .env("OTTOFILE")
                .value_parser(value_parser!(String)),
            Arg::new("list-subtasks")
                .long("list-subtasks")
                .help("List all foreach subtasks and exit")
                .action(clap::ArgAction::SetTrue),
            Arg::new("tasks")
                .long("tasks")
                .help("Print the machine-readable task list and exit")
                .action(clap::ArgAction::SetTrue),
            Arg::new("format")
                .long("format")
                .value_name("FORMAT")
                .help("Output format for --tasks (yaml or json); default: yaml on a tty, json when piped")
                .value_parser(["yaml", "json"]),
            Arg::new("jobs")
                .short('j')
                .long("jobs")
                .value_name("N")
                .help("Number of parallel jobs")
                .default_value(DEFAULT_JOBS.as_str())
                .value_parser(value_parser!(String)),
            Arg::new("tui")
                .short('t')
                .long("tui")
                .help("Enable interactive TUI dashboard for task monitoring")
                .action(clap::ArgAction::SetTrue)
                .global(true),
            Arg::new("no-prefix")
                .long("no-prefix")
                .help("Suppress the [task] prefix on task output")
                .action(clap::ArgAction::SetTrue),
        ]
    }

    fn otto_command() -> Command {
        let mut cmd = Command::new("otto")
            .version(env!("GIT_DESCRIBE"))
            .about("A task runner");
        for arg in Self::global_args() {
            cmd = cmd.arg(arg);
        }
        cmd.allow_external_subcommands(true)
    }

    fn extract_remaining_args(&self, matches: &ArgMatches) -> Vec<String> {
        // Handle external subcommands properly
        if let Some((subcommand_name, sub_matches)) = matches.subcommand() {
            let mut args = vec![subcommand_name.to_string()];

            // For external subcommands, collect all the trailing arguments
            // The key for external subcommand arguments is usually "" (empty string)
            // Note: external subcommands store args as OsString, not String
            if let Some(trailing_args) = sub_matches.get_many::<std::ffi::OsString>("") {
                args.extend(trailing_args.map(|s| s.to_string_lossy().to_string()));
            }

            args
        } else {
            // No subcommand found, return empty
            vec![]
        }
    }

    fn should_show_help(&self, args: &[String]) -> bool {
        // Show help if:
        // 1. Explicit help command: "otto help" or "otto help <task>"
        // 2. Task with --help or -h flag: "otto <task> --help"
        // 3. No args AND no default tasks defined
        if !args.is_empty() {
            if args[0] == "help" {
                return true;
            }
            // Check if --help or -h is present (subcommand help)
            if args.contains(&"--help".to_string()) || args.contains(&"-h".to_string()) {
                return true;
            }
        }

        let default_tasks = &self.config_spec.otto.tasks;
        default_tasks.is_empty()
            || (default_tasks.len() == 1 && default_tasks[0] == "*" && self.config_spec.tasks.is_empty())
    }

    fn show_help(&self, args: &[String]) -> Result<()> {
        if args.is_empty() {
            // Show general help (no default tasks case)
            let mut help_cmd = self.build_help_command();
            help_cmd.print_help()?;
        } else if args.len() == 1 && args[0] == "help" {
            // "otto help" - show general help
            let mut help_cmd = self.build_help_command();
            help_cmd.print_help()?;
        } else if args.len() == 2 && args[0] == "help" {
            // "otto help <task>" - show task-specific help
            let task_name = &args[1];
            self.show_task_help(task_name)?;
        } else if args.contains(&"--help".to_string()) || args.contains(&"-h".to_string()) {
            // "otto <task> --help" or "otto <task> -h" - show task-specific help
            let task_name = &args[0];
            self.show_task_help(task_name)?;
        }
        Ok(())
    }

    fn show_task_help(&self, task_name: &str) -> Result<()> {
        if let Some(task) = self.config_spec.tasks.get(task_name) {
            let mut task_cmd = self.task_to_command_for_help(task);
            task_cmd.print_help()?;
        } else {
            eprintln!("Task '{task_name}' not found");
            std::process::exit(1);
        }
        Ok(())
    }

    /// Print all foreach subtasks and their parent tasks.
    ///
    /// `--list-subtasks` is an enumeration surface: giving the real list is its
    /// job, so it DOES resolve command sources, and it propagates a resolution
    /// failure as its own loud error instead of printing a partial list.
    fn print_subtasks(&self) -> Result<()> {
        let mut has_foreach = false;

        // Sort tasks by name for consistent output
        let mut task_names: Vec<_> = self.config_spec.tasks.keys().collect();
        task_names.sort();

        for task_name in task_names {
            let task_spec = &self.config_spec.tasks[task_name];
            if let Some(ref foreach) = task_spec.foreach {
                has_foreach = true;

                // Get the items for this foreach (resolve relative to ottofile dir)
                let items = match self.resolve_foreach(task_name, foreach) {
                    Ok(items) => items,
                    Err(e) => {
                        if foreach.is_command_source() {
                            return Err(e);
                        }
                        eprintln!("{task_name}: Error resolving items: {e}");
                        continue;
                    }
                };

                println!("{task_name} ({} items):", items.len());
                for item in &items {
                    println!("  {task_name}:{}", item.identifier);
                }
                println!();
            }
        }

        if !has_foreach {
            println!("No tasks with foreach directive found.");
        }

        Ok(())
    }

    fn resolve_default_tasks(&self) -> Result<Vec<String>> {
        let default_tasks = &self.config_spec.otto.tasks;

        if default_tasks.is_empty() {
            return Ok(vec![]); // No default tasks defined
        }

        let mut resolved_tasks = Vec::new();

        for task_pattern in default_tasks {
            if task_pattern == "*" {
                // "*" means all tasks
                resolved_tasks.extend(
                    self.config_spec
                        .tasks
                        .keys()
                        .filter(|name| *name != "graph") // Exclude meta-tasks
                        .cloned(),
                );
            } else {
                // Specific task name
                if self.config_spec.tasks.contains_key(task_pattern) {
                    resolved_tasks.push(task_pattern.clone());
                } else {
                    eprintln!("Warning: Default task '{task_pattern}' not found");
                }
            }
        }

        resolved_tasks.sort();
        resolved_tasks.dedup();

        Ok(resolved_tasks)
    }

    /// Every name `partitions()` may split the arg list on: the ottofile's
    /// tasks, the built-ins, and each foreach task's subtask ids.
    ///
    /// `requested` is the raw arg list. A command-sourced foreach resolves here
    /// only if those args mention it (by parent name or by a subtask-shaped
    /// token); otherwise its subtask ids are not needed to partition this
    /// invocation and the command must not run. When it does resolve, a failure
    /// is returned, not swallowed - the silent `let Ok(items)` that predates
    /// this phase stays only for static sources, whose failures are reported at
    /// the expansion site instead.
    fn get_task_names(&self, requested: &[String]) -> Result<Vec<String>> {
        let mut task_names: Vec<String> = self.config_spec.tasks.keys().cloned().collect();
        task_names.push("graph".to_string()); // Always include built-in tasks
        task_names.push("help".to_string()); // Always include help as a special command

        // Also include expanded subtask names for foreach tasks
        for (name, spec) in &self.config_spec.tasks {
            let Some(ref foreach) = spec.foreach else {
                continue;
            };
            if foreach.is_command_source() {
                if !Self::args_mention_task(requested, name) {
                    continue;
                }
                for item in self.resolve_foreach(name, foreach)? {
                    task_names.push(format!("{}:{}", name, item.identifier));
                }
            } else if let Ok(items) = self.resolve_foreach(name, foreach) {
                for item in items {
                    task_names.push(format!("{}:{}", name, item.identifier));
                }
            }
        }

        Ok(task_names)
    }

    /// The set of ottofile task names this run can reach from `roots`.
    ///
    /// Mirrors `collect_transitive_deps` on the *unexpanded* specs, so it is a
    /// superset of what the run set can contain: upstream via `before:`,
    /// downstream via `after:`, and the inverted `after:` relation (X's
    /// `after: [Y]` makes X a dependency of Y). Subtask-shaped roots collapse
    /// to their parent. Used to decide which command-sourced foreach tasks may
    /// stay unresolved.
    fn reachable_task_names(&self, roots: &[String]) -> HashSet<String> {
        // name -> tasks that must run when `name` runs, via the inverted `after:` edge.
        let mut inverted_after: HashMap<&str, Vec<&str>> = HashMap::new();
        for (name, spec) in &self.config_spec.tasks {
            for edge in &spec.after {
                inverted_after
                    .entry(Self::parent_task_name(&edge.task))
                    .or_default()
                    .push(name.as_str());
            }
        }

        let mut reachable: HashSet<String> = HashSet::new();
        let mut queue: Vec<String> = roots
            .iter()
            .map(|root| Self::parent_task_name(root).to_string())
            .collect();

        while let Some(name) = queue.pop() {
            if !reachable.insert(name.clone()) {
                continue;
            }
            let mut push = |target: &str| queue.push(Self::parent_task_name(target).to_string());
            if let Some(spec) = self.config_spec.tasks.get(&name) {
                for edge in &spec.before {
                    push(&edge.task);
                }
                for edge in &spec.after {
                    push(&edge.task);
                }
            }
            if let Some(dependents) = inverted_after.get(name.as_str()) {
                for dependent in dependents {
                    push(dependent);
                }
            }
        }

        reachable
    }

    /// `up:gamma` -> `up`; anything else unchanged.
    fn parent_task_name(name: &str) -> &str {
        name.split_once(':').map_or(name, |(parent, _)| parent)
    }

    fn extract_task_names_from_partitions(&self) -> Vec<String> {
        self.pargs
            .iter()
            .filter_map(|p| if p.is_empty() { None } else { Some(p[0].clone()) })
            .collect()
    }

    fn process_tasks_with_filter(&self, requested_tasks: &[String]) -> Result<Vec<Task>> {
        // Step 0: Evaluate global environment variables once
        // (memoized: a command-sourced foreach may already have forced this
        // evaluation at partition time, and both sites must see one result)
        let global_envs = self.global_envs().clone();

        // Step 0.4: Check which requested tasks have --Serial flag
        let serial_tasks: HashSet<String> = self.detect_serial_tasks(requested_tasks);

        // Step 0.5: Expand foreach tasks into subtasks
        let mut deferred_foreach: HashSet<String> = HashSet::new();
        let (mut expanded_tasks, serial_membership) =
            self.expand_foreach_tasks_with_serial(&serial_tasks, requested_tasks, &mut deferred_foreach)?;

        // Step 0.6: Desugar `on-failure:` into synthetic `after:` edges.
        // X with `on-failure: [Y]` becomes `X.after += [{task: Y, when: failure}]`,
        // which compute_task_deps_from_specs inverts into Y.task_deps += [{X, failure}].
        Self::apply_on_failure_sugar(&mut expanded_tasks)?;

        // Step 1: Compute all task dependencies using simple linear algorithm
        let task_deps = Self::compute_task_deps_from_specs(&expanded_tasks)?;

        let mut tasks_needed = HashSet::new();
        for task_name in requested_tasks {
            Self::collect_transitive_deps(task_name, &task_deps, &expanded_tasks, &mut tasks_needed)?;
        }

        // The reachability analysis that deferred a command source must be a
        // superset of the run set. If it ever isn't, we'd silently schedule an
        // empty parent instead of the subtasks - fail loudly instead.
        for name in &tasks_needed {
            if deferred_foreach.contains(Self::parent_task_name(name)) {
                return Err(eyre!(
                    "Internal error: task '{}' is in the run set but its command-sourced foreach was \
                     skipped as unreachable; please report this with the ottofile",
                    name
                ));
            }
        }

        // Param resolution uses a multi-phase approach to support propagation:
        //   Phase 1: Apply CLI-provided values only
        //   Phase 2: Propagate values from dependents to dependencies (by name-matching)
        //   Phase 3: Apply defaults for any still-unset params

        // Collect task data with CLI-provided tracking
        let mut task_entries: Vec<(String, Task)> = Vec::new();
        let mut cli_provided_params: HashMap<String, HashSet<String>> = HashMap::new();

        // Phase 1: Create tasks and apply CLI-provided values
        for task_name in &tasks_needed {
            let task_spec = expanded_tasks
                .get(task_name)
                .ok_or_else(|| eyre!("Task '{}' not found", task_name))?;

            // Virtual parent tasks are kept as real (executable) aggregator tasks.
            // Their action is empty; the scheduler short-circuits execution and aggregates
            // their subtasks' statuses to derive the parent's final status.
            let mut task = Task::from_task_with_cwd_and_global_envs(task_spec, &self.cwd, &global_envs);
            let mut cli_provided = HashSet::new();

            // Find the partition for this task's arguments
            let task_args = self.pargs.iter().find(|args| !args.is_empty() && args[0] == *task_name);

            if let Some(args) = task_args
                && args.len() > 1
            {
                // Parse task arguments using clap. Use the original (unexpanded) task spec
                // for clap so foreach-derived flags like `--Serial` are still recognized
                // - the expanded virtual parent has `foreach: None` and would reject the flag.
                let clap_spec = self.config_spec.tasks.get(task_name).unwrap_or(task_spec);
                let task_command = self.task_to_command(clap_spec, BuildMode::Bind)?;
                let matches = task_command.get_matches_from(args);

                for param_spec in task_spec.params.values() {
                    match param_spec.param_type {
                        ParamType::FLG => {
                            // Boolean flag - CLI-provided if user explicitly passed it
                            let flag_value = matches.get_flag(param_spec.name.as_str());
                            if flag_value {
                                cli_provided.insert(param_spec.name.clone());
                                task.values
                                    .insert(param_spec.name.clone(), Value::Item("true".to_string()));
                                let env_name = param_spec.name.replace('-', "_");
                                task.envs.insert(env_name, "true".to_string());
                            }
                            // Don't apply default yet — deferred to Phase 3
                        }
                        ParamType::OPT | ParamType::POS => {
                            // Check if value was provided on CLI vs from clap default
                            if matches.value_source(param_spec.name.as_str())
                                == Some(clap::parser::ValueSource::CommandLine)
                                && let Some(value) = matches.get_one::<String>(param_spec.name.as_str())
                            {
                                cli_provided.insert(param_spec.name.clone());
                                task.values
                                    .insert(param_spec.name.clone(), Value::Item(value.to_string()));
                                let env_name = param_spec.name.replace('-', "_");
                                task.envs.insert(env_name, value.to_string());
                            }
                            // Don't apply default yet — deferred to Phase 3
                        }
                    }
                }
            }
            // No args: no CLI-provided values, defaults applied in Phase 3

            // Override task_deps with computed dependencies
            task.task_deps = task_deps.get(task_name).map(|deps| deps.to_vec()).unwrap_or_default();

            // Carry serial-group membership through to the scheduler's ready loop.
            if let Some((group, index)) = serial_membership.get(task_name) {
                task.serial_group = Some(group.clone());
                task.serial_index = *index;
            }

            cli_provided_params.insert(task_name.clone(), cli_provided);
            task_entries.push((task_name.clone(), task));
        }

        // Phase 2: Propagate values from dependents to dependencies
        // (see param-propagation-design.md for full algorithm)
        self.propagate_params(&expanded_tasks, &task_deps, &mut task_entries, &cli_provided_params)?;

        // Phase 3: Apply defaults for any still-unset params
        for (task_name, task) in &mut task_entries {
            let task_spec = match expanded_tasks.get(task_name) {
                Some(spec) => spec,
                None => continue,
            };
            for param_spec in task_spec.params.values() {
                if task.values.contains_key(&param_spec.name) {
                    continue; // Already set from CLI or propagation
                }
                match param_spec.param_type {
                    ParamType::FLG => {
                        let default_value = param_spec.default.as_deref().unwrap_or("false");
                        task.values
                            .insert(param_spec.name.clone(), Value::Item(default_value.to_string()));
                        let env_name = param_spec.name.replace('-', "_");
                        task.envs.insert(env_name, default_value.to_string());
                    }
                    ParamType::OPT | ParamType::POS => {
                        if let Some(ref default) = param_spec.default {
                            task.values
                                .insert(param_spec.name.clone(), Value::Item(default.clone()));
                            let env_name = param_spec.name.replace('-', "_");
                            task.envs.insert(env_name, default.clone());
                        }
                    }
                }
            }
        }

        let tasks = task_entries.into_iter().map(|(_, task)| task).collect();
        Ok(tasks)
    }

    /// Propagate param values from dependents (parents) to their dependencies.
    ///
    /// When a parent task has a resolved param and its dependency declares a param
    /// with the same name but wasn't given an explicit CLI value, the dependency
    /// inherits the parent's value. Propagation is transitive through chains
    /// (deploy -> middle -> build) as long as each intermediate task declares the
    /// param. Diamond conflicts (two parents propagating different values) are
    /// rejected with a clear error.
    fn propagate_params(
        &self,
        expanded_tasks: &HashMap<String, TaskSpec>,
        task_deps: &HashMap<String, Vec<TaskEdge>>,
        task_entries: &mut [(String, Task)],
        cli_provided_params: &HashMap<String, HashSet<String>>,
    ) -> Result<()> {
        let entry_names: HashSet<String> = task_entries.iter().map(|(n, _)| n.clone()).collect();

        // Build dependency graph filtered to only entry tasks, resolving virtual parents
        let filtered_deps = Self::build_filtered_deps(task_deps, &entry_names, expanded_tasks);

        // Build reverse index: dep -> list of dependents (value sources for propagation)
        let dependents_of = Self::build_reverse_index(&filtered_deps);

        // Topological sort in propagation order (dependents before deps)
        let ordered = Self::topo_sort_propagation_order(&filtered_deps, &entry_names);

        // Build name-to-index map for task_entries (owned keys to avoid borrow conflict)
        let name_to_idx: HashMap<String, usize> = task_entries
            .iter()
            .enumerate()
            .map(|(i, (name, _))| (name.clone(), i))
            .collect();

        // Pre-populate resolved values from CLI-provided params
        let mut resolved_values: HashMap<String, HashMap<String, String>> = HashMap::new();
        for (task_name, task) in task_entries.iter() {
            let mut values = HashMap::new();
            for (param_name, value) in &task.values {
                if let Value::Item(v) = value {
                    values.insert(param_name.clone(), v.clone());
                }
            }
            resolved_values.insert(task_name.clone(), values);
        }

        // Process tasks in propagation order
        let empty_set = HashSet::new();
        for task_name in &ordered {
            let Some(task_spec) = expanded_tasks.get(task_name) else {
                continue;
            };
            let cli_provided = cli_provided_params.get(task_name).unwrap_or(&empty_set);

            for param_spec in task_spec.params.values() {
                if cli_provided.contains(&param_spec.name) {
                    continue; // CLI value takes precedence
                }

                // Collect values from all dependents (parents)
                let mut inherited: Vec<(&str, &str)> = Vec::new();
                if let Some(parents) = dependents_of.get(task_name.as_str()) {
                    for parent_name in parents {
                        if let Some(parent_resolved) = resolved_values.get(*parent_name)
                            && let Some(value) = parent_resolved.get(&param_spec.name)
                        {
                            inherited.push((parent_name, value));
                        }
                    }
                }

                if inherited.is_empty() {
                    continue;
                }

                // Check all parents agree on the value
                let first_value = inherited[0].1;
                if !inherited.iter().all(|(_, v)| *v == first_value) {
                    let details: Vec<String> = inherited
                        .iter()
                        .map(|(parent, value)| format!("  {} provides '{}'", parent, value))
                        .collect();
                    return Err(eyre!(
                        "Conflicting param propagation for '{}' on task '{}':\n{}\n\
                         Hint: Use explicit CLI values to resolve the conflict, \
                         e.g., otto {} --{} <value>",
                        param_spec.name,
                        task_name,
                        details.join("\n"),
                        task_name,
                        param_spec.name,
                    ));
                }

                // Validate choices constraint on propagated value. This is the
                // second bind trigger for a dynamic set: a value the user never
                // typed at this task still has to be valid here, so invoking A
                // can legitimately run dependency B's choices-command. The
                // per-invocation cache is what keeps that at one execution.
                let choices = self.param_choices(task_name, param_spec, BuildMode::Bind)?;
                if !choices.is_empty() && !choices.contains(&first_value.to_string()) {
                    let source_task = inherited[0].0;
                    return Err(eyre!(
                        "Propagated value '{}' for param '{}' on task '{}' (from task '{}') \
                         is not in allowed choices: [{}]",
                        first_value,
                        param_spec.name,
                        task_name,
                        source_task,
                        choices.join(", "),
                    ));
                }

                // Inherit the value
                let value = first_value.to_string();
                resolved_values
                    .entry(task_name.clone())
                    .or_default()
                    .insert(param_spec.name.clone(), value.clone());

                if let Some(&idx) = name_to_idx.get(task_name.as_str()) {
                    let (_, task) = &mut task_entries[idx];
                    task.values.insert(param_spec.name.clone(), Value::Item(value.clone()));
                    let env_name = param_spec.name.replace('-', "_");
                    task.envs.insert(env_name, value);
                }
            }
        }

        Ok(())
    }

    /// Build dependency graph filtered to only include tasks in entry_names,
    /// resolving virtual parent references to their subtasks.
    fn build_filtered_deps(
        task_deps: &HashMap<String, Vec<TaskEdge>>,
        entry_names: &HashSet<String>,
        expanded_tasks: &HashMap<String, TaskSpec>,
    ) -> HashMap<String, Vec<String>> {
        let mut filtered: HashMap<String, Vec<String>> = HashMap::new();
        for name in entry_names {
            let mut deps = Vec::new();
            if let Some(raw_deps) = task_deps.get(name) {
                for dep in raw_deps {
                    if expanded_tasks.get(&dep.task).is_some_and(|s| s.virtual_parent) {
                        // Virtual parent -> redirect to subtasks
                        let prefix = format!("{}:", dep.task);
                        for subtask in entry_names {
                            if subtask.starts_with(&prefix) {
                                deps.push(subtask.clone());
                            }
                        }
                    } else if entry_names.contains(&dep.task) {
                        deps.push(dep.task.clone());
                    }
                }
            }
            filtered.insert(name.clone(), deps);
        }
        filtered
    }

    /// Build reverse index: maps each task to the list of tasks that depend on it
    /// (i.e., its dependents/parents in propagation terminology).
    fn build_reverse_index(filtered_deps: &HashMap<String, Vec<String>>) -> HashMap<&str, Vec<&str>> {
        let mut reverse: HashMap<&str, Vec<&str>> = HashMap::new();
        for (task_name, deps) in filtered_deps {
            for dep in deps {
                reverse.entry(dep.as_str()).or_default().push(task_name.as_str());
            }
        }
        reverse
    }

    /// Topological sort in propagation order: dependents before their deps.
    ///
    /// In the depends-on graph (A -> B means A depends on B), nodes with no tasks
    /// depending on them (in-degree 0) are processed first. These are the leaf
    /// dependents that propagate values downward.
    fn topo_sort_propagation_order(
        filtered_deps: &HashMap<String, Vec<String>>,
        entry_names: &HashSet<String>,
    ) -> Vec<String> {
        use std::collections::VecDeque;

        // Compute in-degree: how many tasks list each task as a dependency
        let mut in_degree: HashMap<&str, usize> = entry_names.iter().map(|n| (n.as_str(), 0)).collect();
        for deps in filtered_deps.values() {
            for dep in deps {
                *in_degree.entry(dep.as_str()).or_default() += 1;
            }
        }

        // Start with nodes that have in-degree 0 (no task depends on them)
        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(&name, _)| name.to_string())
            .collect();
        // Sort for deterministic ordering
        let mut sorted_queue: Vec<String> = queue.drain(..).collect();
        sorted_queue.sort();
        queue.extend(sorted_queue);

        let mut result = Vec::new();
        while let Some(node) = queue.pop_front() {
            result.push(node.clone());
            if let Some(deps) = filtered_deps.get(&node) {
                // Collect and sort for deterministic ordering
                let mut next: Vec<&str> = Vec::new();
                for dep in deps {
                    let count = in_degree.get_mut(dep.as_str()).unwrap();
                    *count -= 1;
                    if *count == 0 {
                        next.push(dep);
                    }
                }
                next.sort();
                for dep in next {
                    queue.push_back(dep.to_string());
                }
            }
        }

        result
    }

    /// Desugar each host's `on_failure:` field into synthetic `after:` edges.
    ///
    /// For each host X with `on_failure: [Y]`:
    /// - Push `EdgeSpec { task: Y, when: Failure, is_injected_sugar: true }` onto X's
    ///   `after:` list. compute_task_deps_from_specs then inverts this to
    ///   `Y.task_deps += [{X, failure}]`, so Y depends on X with `when: failure`.
    /// - The host's `on_failure` field is preserved verbatim so the serializer
    ///   can re-emit it; `is_injected_sugar` lets the serializer filter the
    ///   synthetic `after:` entry so it doesn't show up as a duplicate.
    fn apply_on_failure_sugar(specs: &mut HashMap<String, TaskSpec>) -> Result<()> {
        // Collect (host, target) pairs first, then mutate. We can't hold a &mut to
        // `specs` while iterating it. Sort for determinism: HashMap iteration order
        // is non-deterministic, so without sorting the synthetic-edge order would
        // sporadically reorder between runs and break byte-equivalent round-trip.
        let mut pairs: Vec<(String, String)> = specs
            .iter()
            .flat_map(|(host, spec)| spec.on_failure.iter().map(move |target| (host.clone(), target.clone())))
            .collect();
        pairs.sort();

        for (host, target) in pairs {
            if host == target {
                return Err(eyre!(
                    "on-failure on task '{}' references itself; a task cannot depend on its own failure",
                    host
                ));
            }
            if !specs.contains_key(&target) {
                return Err(eyre!(
                    "on-failure on task '{}' references unknown task '{}'",
                    host,
                    target
                ));
            }
            let host_spec = specs
                .get_mut(&host)
                .expect("host was iterated from specs and the map has not shrunk");
            host_spec.after.push(crate::cfg::edge::EdgeSpec {
                task: target.clone(),
                when: crate::cfg::edge::When::Failure,
                from_sugar: false,
                is_injected_sugar: true,
            });
        }
        Ok(())
    }

    /// Compute task dependencies from a given task specs map.
    /// Validates that all referenced dependencies exist in the task specs.
    fn compute_task_deps_from_specs(task_specs: &HashMap<String, TaskSpec>) -> Result<HashMap<String, Vec<TaskEdge>>> {
        let mut task_deps: HashMap<String, Vec<TaskEdge>> = HashMap::new();

        // Initialize with direct dependencies from 'before' field
        for (task_name, task_spec) in task_specs {
            let edges: Vec<TaskEdge> = task_spec
                .before
                .iter()
                .map(|e| TaskEdge::new(e.task.clone(), e.when))
                .collect();
            task_deps.insert(task_name.clone(), edges);
        }

        // `after` on task X means: every task in X's `after` list depends on X.
        // The condition lives on the inverted edge: X.after = [{task: Y, when: W}]
        // means Y now has a dependency {task: X, when: W} added to Y's task_deps.
        //
        // Dedup is by (task, when) tuple - identical (task, when) pairs collapse to one edge.
        for (task_name, task_spec) in task_specs {
            for after_edge in &task_spec.after {
                let deps = task_deps.entry(after_edge.task.clone()).or_default();
                let new_edge = TaskEdge::new(task_name.clone(), after_edge.when);
                if !deps.iter().any(|d| d.task == new_edge.task && d.when == new_edge.when) {
                    deps.push(new_edge);
                }
            }
        }

        // Validate all dependencies exist
        // This catches typos like "install:tx" when only "install:td" exists
        for (task_name, deps) in &task_deps {
            for dep in deps {
                if !task_specs.contains_key(&dep.task) {
                    return Err(eyre!(
                        "Task '{}' has unknown dependency '{}'\n\
                         Hint: Check for typos in the task name.",
                        task_name,
                        dep.task
                    ));
                }
            }
        }

        Ok(task_deps)
    }

    /// Detect which requested tasks have --Serial flag in their arguments
    fn detect_serial_tasks(&self, requested_tasks: &[String]) -> HashSet<String> {
        let mut serial_tasks = HashSet::new();

        for task_name in requested_tasks {
            // Find partition for this task
            if let Some(args) = self.pargs.iter().find(|args| !args.is_empty() && args[0] == *task_name) {
                // Check if --Serial is present
                if args.contains(&"--Serial".to_string()) {
                    serial_tasks.insert(task_name.clone());
                }
            }
        }

        serial_tasks
    }

    /// Expand all foreach tasks into their subtasks.
    ///
    /// Returns a new task specs map with:
    /// - Non-foreach tasks unchanged
    /// - Foreach tasks replaced by: virtual parent + N subtasks
    ///
    /// When a task is in serial_tasks, its subtasks join a serial group: the returned
    /// membership map records `subtask name -> (group name, order index)`. Serial
    /// ordering is NOT expressed as `before:` edges - "runs after" and "requires" are
    /// different things, and edges are the latter (see the Phase 4 design bullet in
    /// docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md).
    ///
    /// `requested_tasks` gates command-sourced foreach: a task the run can't
    /// reach is left unexpanded so its command never runs (`otto build` must
    /// not execute an unrelated `up` task's command source). `deferred`
    /// collects the names left unexpanded, so the caller can prove none of them
    /// ended up in the run set.
    fn expand_foreach_tasks_with_serial(
        &self,
        serial_tasks: &HashSet<String>,
        requested_tasks: &[String],
        deferred: &mut HashSet<String>,
    ) -> Result<(HashMap<String, TaskSpec>, SerialMembership)> {
        use crate::cfg::task::TaskSpecs;

        let mut expanded: TaskSpecs = HashMap::new();
        let mut membership: SerialMembership = HashMap::new();
        let reachable = self.reachable_task_names(requested_tasks);

        for (name, spec) in &self.config_spec.tasks {
            if spec.has_foreach() {
                let foreach = spec.foreach.as_ref().expect("has_foreach() implies Some");

                // Lazy trigger: a command source resolves only for a task this
                // run can reach. Everything else keeps its virtual parent as a
                // placeholder and never executes anything.
                if foreach.is_command_source() && !reachable.contains(name) {
                    deferred.insert(name.clone());
                    let mut parent = spec.as_virtual_parent();
                    parent.before = Vec::new();
                    expanded.insert(name.clone(), parent);
                    continue;
                }

                // Expand foreach task into subtasks (resolve relative to ottofile dir)
                let items = self.resolve_foreach(name, foreach)?;
                let subtasks = spec.expand_foreach_with_items(&items)?;

                if subtasks.is_empty() {
                    // Zero matches - just keep the virtual parent for dependency tracking
                    log::warn!("foreach task '{}' expanded to 0 subtasks", name);
                }

                // Check if this task should run serially (CLI --Serial flag OR config parallel: false)
                let run_serial = serial_tasks.contains(name) || spec.foreach.as_ref().is_some_and(|f| !f.parallel);

                // Build virtual parent. Its `before:` is replaced with When::Always edges
                // to every subtask, so the scheduler queues the parent only after all
                // subtasks reach a terminal state (Completed | Failed | Skipped).
                let mut parent = spec.as_virtual_parent();
                parent.before = subtasks
                    .iter()
                    .map(|st| crate::cfg::edge::EdgeSpec {
                        task: st.name.clone(),
                        when: crate::cfg::edge::When::Always,
                        from_sugar: false,
                        is_injected_sugar: false,
                    })
                    .collect();

                // Add all subtasks. Each subtask inherits the *original* parent's
                // before dependencies (its prerequisites), not the rewritten one.
                // Subtasks do NOT inherit the parent's `after:` edges - only the
                // parent triggers downstreams; otherwise every subtask would
                // double-fire the downstream task.
                for (index, mut subtask) in subtasks.into_iter().enumerate() {
                    // Subtask inherits parent's original before dependencies
                    subtask.before = spec.before.clone();
                    subtask.after = Vec::new();

                    // Serial ordering is recorded as group membership, not as an edge.
                    if run_serial {
                        membership.insert(subtask.name.clone(), (name.clone(), index));
                    }

                    expanded.insert(subtask.name.clone(), subtask);
                }

                expanded.insert(name.clone(), parent);
            } else {
                // Non-foreach task - keep as-is
                expanded.insert(name.clone(), spec.clone());
            }
        }

        Ok((expanded, membership))
    }

    /// Collect all tasks needed to run a given task, including:
    /// - Transitive dependencies (before/upstream tasks)
    /// - After tasks (downstream tasks that should auto-run)
    /// - Subtasks (for foreach parent tasks)
    fn collect_transitive_deps(
        task_name: &str,
        task_deps: &HashMap<String, Vec<TaskEdge>>,
        task_specs: &HashMap<String, TaskSpec>,
        collected: &mut HashSet<String>,
    ) -> Result<()> {
        if collected.contains(task_name) {
            return Ok(());
        }

        collected.insert(task_name.to_string());

        // Collect upstream dependencies (before)
        if let Some(deps) = task_deps.get(task_name) {
            for dep in deps {
                Self::collect_transitive_deps(&dep.task, task_deps, task_specs, collected)?;
            }
        }

        // Collect downstream tasks (after) - these auto-run when this task is requested
        if let Some(spec) = task_specs.get(task_name) {
            for after_edge in &spec.after {
                Self::collect_transitive_deps(&after_edge.task, task_deps, task_specs, collected)?;
            }
        }

        // Collect subtasks for foreach parent tasks
        // Only expand subtasks if this is a parent task (no colon in name)
        // If user requests "install:td", don't also collect install:ts, install:cs
        if !task_name.contains(':') {
            let prefix = format!("{}:", task_name);
            for subtask_name in task_specs.keys() {
                if subtask_name.starts_with(&prefix) {
                    Self::collect_transitive_deps(subtask_name, task_deps, task_specs, collected)?;
                }
            }
        }

        Ok(())
    }

    /// Build a clap `Command` for a task.
    ///
    /// The mode is explicit because the two callers want opposite things from a
    /// dynamic source, and the distinction used to ride on `cwd: Option<&Path>`
    /// (help passed `Some`, bind passed `None`) - an implicit discriminator that
    /// Phase 6b inverts, since the bind path is the one that needs the ottofile
    /// directory and the resolver. Both modes now read `self`, so the mode says
    /// what it means: `Help` describes dynamic sources, `Bind` resolves them.
    fn task_to_command(&self, task_spec: &TaskSpec, mode: BuildMode) -> Result<Command> {
        let mut cmd = Command::new(task_spec.name.clone());

        // Build help text, including foreach count if available
        let help_text = if let Some(ref foreach) = task_spec.foreach {
            // A command source is never executed to render help: the item count
            // isn't knowable without running user code, so help says so and
            // stays execution-free (design doc Phase 6, and the same rule
            // Phase 6b applies to dynamic param choices).
            let foreach_indicator = if foreach.is_command_source() {
                " [dynamic]".to_string()
            } else if mode == BuildMode::Help {
                let count = foreach.resolve_items(self.base_dir()).map_or(0, |items| items.len());
                if count > 0 { format!(" [{count} items]") } else { " [foreach]".to_string() }
            } else {
                // Binding never renders this string; don't walk the filesystem for it.
                " [foreach]".to_string()
            };

            match &task_spec.help {
                Some(help) => format!("{help}{foreach_indicator}"),
                None => foreach_indicator,
            }
        } else {
            task_spec.help.clone().unwrap_or_default()
        };

        if !help_text.is_empty() {
            cmd = cmd.about(help_text);
        }

        for param_spec in task_spec.params.values() {
            let arg = self.param_to_arg(&task_spec.name, param_spec, mode)?;
            cmd = cmd.arg(arg);
        }

        // Auto-inject --Serial flag for foreach tasks
        if task_spec.has_foreach() {
            cmd = cmd.arg(
                Arg::new("Serial")
                    .long("Serial")
                    .help("[builtin] Run subtasks sequentially instead of in parallel")
                    .action(clap::ArgAction::SetTrue),
            );
        }

        Ok(cmd)
    }

    /// Help-mode wrapper: rendering help can never fail, because it never runs
    /// anything. Keeping the infallible signature makes that guarantee visible
    /// at every help call site.
    fn task_to_command_for_help(&self, task_spec: &TaskSpec) -> Command {
        self.task_to_command(task_spec, BuildMode::Help)
            .expect("help mode resolves no dynamic sources and cannot fail")
    }

    fn param_to_arg(&self, task_name: &str, param_spec: &ParamSpec, mode: BuildMode) -> Result<Arg> {
        let mut arg = Arg::new(param_spec.name.clone());

        if let Some(short) = param_spec.short {
            arg = arg.short(short);
        }

        if let Some(ref long) = param_spec.long {
            arg = arg.long(long.clone());
        }

        // A dynamic value set can't be listed in help without executing the
        // command, so help names the command instead and a human can run it.
        let help_text = match (&param_spec.help, &param_spec.choices_command) {
            (Some(help), Some(command)) => Some(format!("{help} [dynamic choices: {command}]")),
            (None, Some(command)) => Some(format!("[dynamic choices: {command}]")),
            (help, None) => help.clone(),
        };
        if let Some(help) = help_text {
            arg = arg.help(help);
        }

        // Handle different parameter types
        match param_spec.param_type {
            ParamType::FLG => {
                // Boolean flag - no value required
                arg = arg.action(clap::ArgAction::SetTrue);
            }
            ParamType::OPT | ParamType::POS => {
                // Argument with value
                arg = arg.value_parser(value_parser!(String));

                if let Some(ref default) = param_spec.default {
                    arg = arg.default_value(default.clone());
                }

                let choices = self.param_choices(task_name, param_spec, mode)?;
                if !choices.is_empty() {
                    arg = arg.value_parser(clap::builder::PossibleValuesParser::new(choices));
                }
            }
        }

        // Handle positional arguments
        if param_spec.param_type == ParamType::POS {
            let value_name = param_spec
                .metavar
                .as_deref()
                .unwrap_or(param_spec.name.as_str())
                .to_string();
            arg = arg.value_name(value_name);
        }

        Ok(arg)
    }

    /// The allowed value set for a param, resolving a `choices-command:` source
    /// at most once per invocation.
    ///
    /// In `Help` mode a dynamic source yields no set at all: help must execute
    /// nothing, so it renders `[dynamic choices: ...]` and leaves the value
    /// unconstrained (nothing is being validated on a help path anyway). Both
    /// bind triggers - direct invocation here, propagated-value validation in
    /// `propagate_params` - come through this one function, which is why they
    /// share the cache instead of running the command twice.
    fn param_choices(&self, task_name: &str, param_spec: &ParamSpec, mode: BuildMode) -> Result<Vec<String>> {
        if !param_spec.has_dynamic_choices() {
            return Ok(param_spec.choices.clone());
        }
        if mode == BuildMode::Help {
            return Ok(Vec::new());
        }
        let key = format!("{task_name}:{}", param_spec.name);
        self.resolver.choices(&key, || {
            param_spec.resolve_choices_command(task_name, self.base_dir(), self.global_envs())
        })
    }

    fn build_help_command(&self) -> Command {
        let mut cmd = Command::new("otto")
            .version(env!("GIT_DESCRIBE"))
            .about("A task runner");
        for arg in Self::global_args() {
            cmd = cmd.arg(arg);
        }
        cmd = cmd.allow_external_subcommands(true);

        if !self.config_spec.tasks.is_empty() {
            // Separate regular tasks from built-in commands
            let mut regular_tasks: Vec<_> = self
                .config_spec
                .tasks
                .iter()
                .filter(|(name, _)| !BUILTIN_COMMANDS.contains(&name.as_str()))
                .collect();
            regular_tasks.sort_by_key(|(name, _)| name.as_str());

            for (_, task_spec) in regular_tasks {
                cmd = cmd.subcommand(self.task_to_command_for_help(task_spec));
            }

            // Collect and sort built-in commands
            let mut builtins: Vec<(&String, &TaskSpec)> = self
                .config_spec
                .tasks
                .iter()
                .filter(|(name, _)| BUILTIN_COMMANDS.contains(&name.as_str()))
                .collect();
            builtins.sort_by_key(|(name, _)| name.as_str());

            for (_, task_spec) in builtins {
                cmd = cmd.subcommand(self.task_to_command_for_help(task_spec));
            }
        } else {
            cmd = cmd.after_help(ottofile_not_found_message());
        }

        cmd
    }

    /// Global flags only, no epilogue: the shared base for both config-failure
    /// help fallbacks. Split out so the "found but unparseable" path can render
    /// the same flag list without inheriting the not-found claim.
    fn build_bare_help_command() -> Command {
        let mut cmd = Command::new("otto")
            .version(env!("GIT_DESCRIBE"))
            .about("A task runner");
        for arg in Self::global_args() {
            cmd = cmd.arg(arg);
        }
        cmd.allow_external_subcommands(true)
    }

    /// The fallback for the "no ottofile anywhere up the tree" state only.
    fn build_help_command_with_error() -> Command {
        Self::build_bare_help_command().after_help(ottofile_not_found_message())
    }

    fn inject_graph_meta_task(&mut self) {
        use crate::cfg::param::{Nargs, ParamType};

        let graph_task = TaskSpec {
            name: "Graph".to_string(),
            help: Some("[built-in] Visualize the task dependency graph".to_string()),
            after: vec![],
            before: vec![],
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: {
                let mut params = HashMap::new();

                params.insert(
                    "format".to_string(),
                    ParamSpec {
                        name: "format".to_string(),
                        short: Some('f'),
                        long: Some("format".to_string()),
                        param_type: ParamType::OPT,
                        dest: None,
                        metavar: None,
                        default: Some("ascii".to_string()),
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![
                            "ascii".to_string(),
                            "dot".to_string(),
                            "svg".to_string(),
                            "png".to_string(),
                            "pdf".to_string(),
                        ],
                        nargs: Nargs::One,
                        help: Some("Output format".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "output".to_string(),
                    ParamSpec {
                        name: "output".to_string(),
                        short: None,
                        long: Some("output".to_string()),
                        param_type: ParamType::OPT,
                        dest: None,
                        metavar: None,
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Output file path".to_string()),
                        value: Value::Empty,
                    },
                );

                params
            },
            action: "# Built-in graph command".to_string(),
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        };

        self.config_spec.tasks.insert("Graph".to_string(), graph_task);
    }

    fn inject_clean_meta_task(&mut self) {
        use crate::cfg::param::{Nargs, ParamType};

        let clean_task = TaskSpec {
            name: "Clean".to_string(),
            help: Some("[built-in] Clean old runs from ~/.otto/".to_string()),
            after: vec![],
            before: vec![],
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: {
                let mut params = HashMap::new();

                params.insert(
                    "keep".to_string(),
                    ParamSpec {
                        name: "keep".to_string(),
                        short: None,
                        long: Some("keep".to_string()),
                        param_type: ParamType::OPT,
                        dest: None,
                        metavar: Some("DAYS".to_string()),
                        default: Some("30".to_string()),
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Keep runs from the last N days".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "dry-run".to_string(),
                    ParamSpec {
                        name: "dry-run".to_string(),
                        short: None,
                        long: Some("dry-run".to_string()),
                        param_type: ParamType::FLG,
                        dest: None,
                        metavar: None,
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Show what would be deleted without actually deleting".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "project".to_string(),
                    ParamSpec {
                        name: "project".to_string(),
                        short: None,
                        long: Some("project".to_string()),
                        param_type: ParamType::OPT,
                        dest: None,
                        metavar: Some("HASH".to_string()),
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Only clean runs for a specific project".to_string()),
                        value: Value::Empty,
                    },
                );

                params
            },
            action: "# Built-in clean command".to_string(),
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        };

        self.config_spec.tasks.insert("Clean".to_string(), clean_task);
    }

    fn inject_history_meta_task(&mut self) {
        use crate::cfg::param::{Nargs, ParamType};

        let history_task = TaskSpec {
            name: "History".to_string(),
            help: Some("[built-in] View execution history".to_string()),
            after: vec![],
            before: vec![],
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: {
                let mut params = HashMap::new();

                params.insert(
                    "task".to_string(),
                    ParamSpec {
                        name: "task".to_string(),
                        short: Some('t'),
                        long: Some("task".to_string()),
                        param_type: ParamType::OPT,
                        dest: None,
                        metavar: Some("TASK".to_string()),
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Show history for a specific task".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "limit".to_string(),
                    ParamSpec {
                        name: "limit".to_string(),
                        short: Some('n'),
                        long: Some("limit".to_string()),
                        param_type: ParamType::OPT,
                        dest: None,
                        metavar: Some("N".to_string()),
                        default: Some("20".to_string()),
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Limit number of results".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "status".to_string(),
                    ParamSpec {
                        name: "status".to_string(),
                        short: Some('s'),
                        long: Some("status".to_string()),
                        param_type: ParamType::OPT,
                        dest: None,
                        metavar: Some("STATUS".to_string()),
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec!["success".to_string(), "failed".to_string(), "running".to_string()],
                        nargs: Nargs::One,
                        help: Some("Filter by status".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "project".to_string(),
                    ParamSpec {
                        name: "project".to_string(),
                        short: Some('p'),
                        long: Some("project".to_string()),
                        param_type: ParamType::OPT,
                        dest: None,
                        metavar: Some("HASH".to_string()),
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Filter by project hash".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "json".to_string(),
                    ParamSpec {
                        name: "json".to_string(),
                        short: None,
                        long: Some("json".to_string()),
                        param_type: ParamType::FLG,
                        dest: None,
                        metavar: None,
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Output as JSON".to_string()),
                        value: Value::Empty,
                    },
                );

                params
            },
            action: "# Built-in history command".to_string(),
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        };

        self.config_spec.tasks.insert("History".to_string(), history_task);
    }

    fn inject_stats_meta_task(&mut self) {
        use crate::cfg::param::{Nargs, ParamType};

        let stats_task = TaskSpec {
            name: "Stats".to_string(),
            help: Some("[built-in] View execution statistics".to_string()),
            after: vec![],
            before: vec![],
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: {
                let mut params = HashMap::new();

                params.insert(
                    "task".to_string(),
                    ParamSpec {
                        name: "task".to_string(),
                        short: Some('t'),
                        long: Some("task".to_string()),
                        param_type: ParamType::OPT,
                        dest: None,
                        metavar: Some("TASK".to_string()),
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Show stats for a specific task".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "limit".to_string(),
                    ParamSpec {
                        name: "limit".to_string(),
                        short: Some('n'),
                        long: Some("limit".to_string()),
                        param_type: ParamType::OPT,
                        dest: None,
                        metavar: Some("N".to_string()),
                        default: Some("10".to_string()),
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Limit number of tasks shown".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "json".to_string(),
                    ParamSpec {
                        name: "json".to_string(),
                        short: None,
                        long: Some("json".to_string()),
                        param_type: ParamType::FLG,
                        dest: None,
                        metavar: None,
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Output as JSON".to_string()),
                        value: Value::Empty,
                    },
                );

                params
            },
            action: "# Built-in stats command".to_string(),
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        };

        self.config_spec.tasks.insert("Stats".to_string(), stats_task);
    }

    fn inject_convert_meta_task(&mut self) {
        use crate::cfg::param::{Nargs, ParamType};

        let convert_task = TaskSpec {
            name: "Convert".to_string(),
            help: Some("[built-in] Convert Makefile to Otto YAML format".to_string()),
            after: vec![],
            before: vec![],
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: {
                let mut params = HashMap::new();

                params.insert(
                    "strict".to_string(),
                    ParamSpec {
                        name: "strict".to_string(),
                        short: None,
                        long: Some("strict".to_string()),
                        param_type: ParamType::FLG,
                        dest: None,
                        metavar: None,
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Treat warnings as errors".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "output".to_string(),
                    ParamSpec {
                        name: "output".to_string(),
                        short: Some('o'),
                        long: Some("output".to_string()),
                        param_type: ParamType::OPT,
                        dest: None,
                        metavar: Some("FILE".to_string()),
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Output file (default: stdout)".to_string()),
                        value: Value::Empty,
                    },
                );

                params
            },
            action: "# Built-in convert command".to_string(),
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        };

        self.config_spec.tasks.insert("Convert".to_string(), convert_task);
    }

    fn inject_upgrade_meta_task(&mut self) {
        use crate::cfg::param::{Nargs, ParamType};

        let upgrade_task = TaskSpec {
            name: "Upgrade".to_string(),
            help: Some("[built-in] Upgrade Otto to a newer version".to_string()),
            after: vec![],
            before: vec![],
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: {
                let mut params = HashMap::new();

                params.insert(
                    "dry-run".to_string(),
                    ParamSpec {
                        name: "dry-run".to_string(),
                        short: None,
                        long: Some("dry-run".to_string()),
                        param_type: ParamType::FLG,
                        dest: None,
                        metavar: None,
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Show what would be done without doing it".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "version".to_string(),
                    ParamSpec {
                        name: "version".to_string(),
                        short: Some('v'),
                        long: Some("version".to_string()),
                        param_type: ParamType::OPT,
                        dest: None,
                        metavar: Some("VERSION".to_string()),
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Specific version to upgrade to".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "list-versions".to_string(),
                    ParamSpec {
                        name: "list-versions".to_string(),
                        short: None,
                        long: Some("list-versions".to_string()),
                        param_type: ParamType::FLG,
                        dest: None,
                        metavar: None,
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("List available versions".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "rollback".to_string(),
                    ParamSpec {
                        name: "rollback".to_string(),
                        short: None,
                        long: Some("rollback".to_string()),
                        param_type: ParamType::FLG,
                        dest: None,
                        metavar: None,
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Rollback to previous version".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "force".to_string(),
                    ParamSpec {
                        name: "force".to_string(),
                        short: None,
                        long: Some("force".to_string()),
                        param_type: ParamType::FLG,
                        dest: None,
                        metavar: None,
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Force upgrade even if already on target version".to_string()),
                        value: Value::Empty,
                    },
                );

                params.insert(
                    "no-backup".to_string(),
                    ParamSpec {
                        name: "no-backup".to_string(),
                        short: None,
                        long: Some("no-backup".to_string()),
                        param_type: ParamType::FLG,
                        dest: None,
                        metavar: None,
                        default: None,
                        constant: Value::Empty,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Skip creating backup".to_string()),
                        value: Value::Empty,
                    },
                );

                params
            },
            action: "# Built-in upgrade command".to_string(),
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        };

        self.config_spec.tasks.insert("Upgrade".to_string(), upgrade_task);
    }

    fn inject_builtin_commands(&mut self) {
        self.inject_clean_meta_task();
        self.inject_convert_meta_task();
        self.inject_graph_meta_task();
        self.inject_history_meta_task();
        self.inject_stats_meta_task();
        self.inject_upgrade_meta_task();
    }

    fn find_ottofile(path: &Path) -> Result<Option<PathBuf>> {
        for ottofile in OTTOFILES {
            let ottofile_path = path.join(ottofile);
            if ottofile_path.exists() {
                return Ok(Some(ottofile_path));
            }
        }
        // If we've reached the root, stop searching
        if let Some(parent) = path.parent() {
            if parent == path {
                return Ok(None);
            }
            // Recurse up
            Self::find_ottofile(parent)
        } else {
            Ok(None)
        }
    }

    fn divine_ottofile(value: String) -> Result<Option<PathBuf>> {
        let mut path = expanduser(value)?;
        path = fs::canonicalize(path)?;
        if path.is_dir() {
            return Self::find_ottofile(&path);
        }
        Ok(Some(path))
    }

    fn load_config_from_path(ottofile_path: Option<PathBuf>) -> Result<(ConfigSpec, String, Option<PathBuf>)> {
        if let Some(ottofile) = ottofile_path {
            let content = fs::read_to_string(&ottofile)?;
            let mut hasher = Sha256::new();
            hasher.update(&content);
            let result = hasher.finalize();
            let hash = hex::encode(result)[..8].to_string();

            // Version gate BEFORE the typed parse, against the same string:
            // reversed, a file from a newer otto reports whichever key it
            // added instead of telling the operator to upgrade.
            crate::cfg::otto::check_api_version(&content)?;

            let config_spec: ConfigSpec = serde_yaml::from_str(&content)?;

            // Validate that no tasks use reserved builtin param names
            Self::validate_no_builtin_params(&config_spec)?;

            // Validate foreach sources (a `command:` source is exclusive with
            // glob/items/range). Shape-only, executes nothing, so every
            // surface including `--help` reports the misconfiguration.
            Self::validate_foreach_sources(&config_spec)?;

            Ok((config_spec, hash, Some(ottofile)))
        } else {
            Err(eyre!("{}", ottofile_not_found_message()))
        }
    }

    fn validate_foreach_sources(config: &ConfigSpec) -> Result<()> {
        for (task_name, task_spec) in &config.tasks {
            if let Some(foreach) = &task_spec.foreach {
                foreach.validate_sources(task_name)?;
            }
        }
        Ok(())
    }

    fn validate_no_builtin_params(config: &ConfigSpec) -> Result<()> {
        use crate::cli::builtins::is_builtin_param;

        for (task_name, task_spec) in &config.tasks {
            for param_name in task_spec.params.keys() {
                if is_builtin_param(param_name) {
                    return Err(eyre!(
                        "Task '{}' defines reserved builtin param '--{}'. \
                         Capitalized params are reserved for otto builtins.",
                        task_name,
                        param_name
                    ));
                }
            }
        }
        Ok(())
    }
}

fn indices(args: &[String], task_names: &[String]) -> Vec<usize> {
    let mut indices = vec![];
    for (i, arg) in args.iter().enumerate() {
        if task_names.contains(arg) {
            indices.push(i);
        }
    }
    indices
}

fn partitions(args: &[String], task_names: &[String]) -> Vec<Vec<String>> {
    let task_indices = indices(args, task_names);
    if task_indices.is_empty() {
        return vec![];
    }

    let mut partitions = vec![];
    let mut end = args.len();

    for &index in task_indices.iter().rev() {
        partitions.insert(0, args[index..end].to_vec());
        end = index;
    }

    partitions
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(indices(&args, &task_names), expected);
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
        assert_eq!(partitions(&args, &task_names), expected);
    }

    #[test]
    fn test_partitions_empty() {
        let args = vec!["arg1".to_string(), "arg2".to_string()];
        let task_names = vec!["task1".to_string(), "task2".to_string()];
        let expected: Vec<Vec<String>> = vec![];
        assert_eq!(partitions(&args, &task_names), expected);
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

        assert_eq!(partitions(&args, &task_names), expected);
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
            dest: None,
            metavar: None,
            default,
            constant: Value::Empty,
            choices_command: None,
            choices: vec![],
            nargs: Nargs::default(),
            help: Some(format!("Help for {name}")),
            value: Value::Empty,
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
        // Test that DEFAULT_JOBS equals num_cpus::get()
        let expected = num_cpus::get().to_string();
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
        let (_, _, _, jobs, _, _) = result.unwrap();
        assert_eq!(jobs, 4);
    }

    #[test]
    fn test_jobs_parameter_default() {
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ottofile_path = temp_dir.path().join("otto.yml");
        fs::write(&ottofile_path, "tasks:\n  test:\n    action: echo test\n").unwrap();

        // Test without explicit jobs value (should default to num_cpus::get())
        let args = vec![
            "otto".to_string(),
            "--ottofile".to_string(),
            ottofile_path.to_string_lossy().to_string(),
            "test".to_string(),
        ];

        let mut parser = Parser::new(args).unwrap();
        let result = parser.parse();
        assert!(result.is_ok());
        let (_, _, _, jobs, _, _) = result.unwrap();
        assert_eq!(jobs, num_cpus::get());
    }

    #[test]
    fn test_jobs_parameter_invalid() {
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ottofile_path = temp_dir.path().join("otto.yml");
        fs::write(&ottofile_path, "tasks:\n  test:\n    action: echo test\n").unwrap();

        // Test with invalid jobs value (should fall back to num_cpus::get())
        let args = vec![
            "otto".to_string(),
            "-j".to_string(),
            "invalid".to_string(),
            "--ottofile".to_string(),
            ottofile_path.to_string_lossy().to_string(),
            "test".to_string(),
        ];

        let mut parser = Parser::new(args).unwrap();
        let result = parser.parse();
        assert!(result.is_ok());
        let (_, _, _, jobs, _, _) = result.unwrap();
        assert_eq!(jobs, num_cpus::get());
    }

    // Tests for collect_transitive_deps and after semantic
    #[test]
    fn test_collect_transitive_deps_basic() {
        let mut task_deps = HashMap::new();
        task_deps.insert("a".to_string(), vec![]);
        task_deps.insert("b".to_string(), vec![TaskEdge::success("a")]);
        task_deps.insert("c".to_string(), vec![TaskEdge::success("b")]);

        let task_specs = HashMap::new();
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

        let mut task_specs = HashMap::new();
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

        let mut task_specs = HashMap::new();

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

        let mut task_specs = HashMap::new();

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

        let mut task_specs = HashMap::new();

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
        let mut task_spec = TaskSpec::new(
            "up".to_string(),
            Some("Bring up each service".to_string()),
            vec![],
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            crate::cfg::param::ParamSpecs::new(),
            "echo ${svc}".to_string(),
        );
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
        let mut task_spec = TaskSpec::new(
            "up".to_string(),
            Some("Bring up each service".to_string()),
            vec![],
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            crate::cfg::param::ParamSpecs::new(),
            "echo ${svc}".to_string(),
        );
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
        let (tasks, _, _, _, _, _) = result;

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

    #[test]
    fn test_foreach_subtasks_not_chained_when_parallel_true() {
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ottofile_path = temp_dir.path().join("otto.yml");

        // Create an ottofile with parallel: true (explicit, same as default)
        let config = r#"
tasks:
  install:
    foreach:
      items: [a, b, c]
      as: pkg
      parallel: true
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
        let (tasks, _, _, _, _, _) = result;

        // Find the subtasks
        let subtask_a = tasks.iter().find(|t| t.name == "install:a");
        let subtask_b = tasks.iter().find(|t| t.name == "install:b");
        let subtask_c = tasks.iter().find(|t| t.name == "install:c");

        assert!(subtask_a.is_some(), "subtask install:a should exist");
        assert!(subtask_b.is_some(), "subtask install:b should exist");
        assert!(subtask_c.is_some(), "subtask install:c should exist");

        // With parallel: true, subtasks should NOT be chained
        let b = subtask_b.unwrap();
        let c = subtask_c.unwrap();

        // b should NOT depend on a, c should NOT depend on b
        assert!(
            !b.task_deps.iter().any(|d| d.task == "install:a"),
            "install:b should NOT depend on install:a when parallel: true, got: {:?}",
            b.task_deps
        );
        assert!(
            !c.task_deps.iter().any(|d| d.task == "install:b"),
            "install:c should NOT depend on install:b when parallel: true, got: {:?}",
            c.task_deps
        );

        // ...and they join no serial group, so the scheduler's ordering gate is inert.
        assert_eq!(b.serial_group, None);
        assert_eq!(c.serial_group, None);
    }

    #[test]
    fn test_foreach_subtasks_parallel_by_default() {
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ottofile_path = temp_dir.path().join("otto.yml");

        // Create an ottofile WITHOUT specifying parallel (should default to true)
        let config = r#"
tasks:
  install:
    foreach:
      items: [a, b, c]
      as: pkg
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
        let (tasks, _, _, _, _, _) = result;

        // Find subtask b
        let subtask_b = tasks.iter().find(|t| t.name == "install:b").unwrap();

        // Default (parallel: true) means b should NOT depend on a
        assert!(
            !subtask_b.task_deps.iter().any(|d| d.task == "install:a"),
            "By default, install:b should NOT depend on install:a, got: {:?}",
            subtask_b.task_deps
        );
    }

    // Tests for subtask targeting (task:subtask notation)

    #[test]
    fn test_collect_transitive_deps_parent_expands_subtasks() {
        // When running parent task "install", all subtasks should be collected

        let task_deps = HashMap::new();

        let mut task_specs = HashMap::new();

        // Virtual parent task
        let parent_spec = TaskSpec {
            name: "install".to_string(),
            action: String::new(), // Virtual parent has no action
            ..Default::default()
        };
        task_specs.insert("install".to_string(), parent_spec);

        // Subtasks
        let subtask_td = TaskSpec {
            name: "install:td".to_string(),
            action: "echo td".to_string(),
            ..Default::default()
        };
        task_specs.insert("install:td".to_string(), subtask_td);

        let subtask_ts = TaskSpec {
            name: "install:ts".to_string(),
            action: "echo ts".to_string(),
            ..Default::default()
        };
        task_specs.insert("install:ts".to_string(), subtask_ts);

        let subtask_cs = TaskSpec {
            name: "install:cs".to_string(),
            action: "echo cs".to_string(),
            ..Default::default()
        };
        task_specs.insert("install:cs".to_string(), subtask_cs);

        let mut collected = HashSet::new();

        // Running "install" (parent) should expand to all subtasks
        Parser::collect_transitive_deps("install", &task_deps, &task_specs, &mut collected).unwrap();

        assert!(collected.contains("install"), "parent should be collected");
        assert!(collected.contains("install:td"), "subtask td should be collected");
        assert!(collected.contains("install:ts"), "subtask ts should be collected");
        assert!(collected.contains("install:cs"), "subtask cs should be collected");
        assert_eq!(collected.len(), 4);
    }

    #[test]
    fn test_collect_transitive_deps_subtask_does_not_expand_siblings() {
        // When running a specific subtask "install:td", should NOT collect siblings
        let task_deps = HashMap::new();

        let mut task_specs = HashMap::new();

        // Virtual parent task
        let parent_spec = TaskSpec {
            name: "install".to_string(),
            action: String::new(),
            ..Default::default()
        };
        task_specs.insert("install".to_string(), parent_spec);

        // Subtasks
        let subtask_td = TaskSpec {
            name: "install:td".to_string(),
            action: "echo td".to_string(),
            ..Default::default()
        };
        task_specs.insert("install:td".to_string(), subtask_td);

        let subtask_ts = TaskSpec {
            name: "install:ts".to_string(),
            action: "echo ts".to_string(),
            ..Default::default()
        };
        task_specs.insert("install:ts".to_string(), subtask_ts);

        let subtask_cs = TaskSpec {
            name: "install:cs".to_string(),
            action: "echo cs".to_string(),
            ..Default::default()
        };
        task_specs.insert("install:cs".to_string(), subtask_cs);

        let mut collected = HashSet::new();

        // Running "install:td" should NOT expand to sibling subtasks
        Parser::collect_transitive_deps("install:td", &task_deps, &task_specs, &mut collected).unwrap();

        assert!(
            collected.contains("install:td"),
            "requested subtask should be collected"
        );
        assert!(
            !collected.contains("install:ts"),
            "sibling subtask ts should NOT be collected"
        );
        assert!(
            !collected.contains("install:cs"),
            "sibling subtask cs should NOT be collected"
        );
        assert!(!collected.contains("install"), "parent should NOT be collected");
        assert_eq!(collected.len(), 1);
    }

    #[test]
    fn test_collect_transitive_deps_subtask_with_deps() {
        // Subtask with its own dependencies should still collect those
        let mut task_deps = HashMap::new();
        task_deps.insert("install:td".to_string(), vec![TaskEdge::success("setup")]);
        task_deps.insert("setup".to_string(), vec![]);

        let mut task_specs = HashMap::new();

        let parent_spec = TaskSpec {
            name: "install".to_string(),
            action: String::new(),
            ..Default::default()
        };
        task_specs.insert("install".to_string(), parent_spec);

        let subtask_td = TaskSpec {
            name: "install:td".to_string(),
            action: "echo td".to_string(),
            ..Default::default()
        };
        task_specs.insert("install:td".to_string(), subtask_td);

        let subtask_ts = TaskSpec {
            name: "install:ts".to_string(),
            action: "echo ts".to_string(),
            ..Default::default()
        };
        task_specs.insert("install:ts".to_string(), subtask_ts);

        let setup_spec = TaskSpec {
            name: "setup".to_string(),
            action: "echo setup".to_string(),
            ..Default::default()
        };
        task_specs.insert("setup".to_string(), setup_spec);

        let mut collected = HashSet::new();

        // Running "install:td" should collect its dependency "setup" but NOT sibling subtasks
        Parser::collect_transitive_deps("install:td", &task_deps, &task_specs, &mut collected).unwrap();

        assert!(
            collected.contains("install:td"),
            "requested subtask should be collected"
        );
        assert!(collected.contains("setup"), "dependency should be collected");
        assert!(
            !collected.contains("install:ts"),
            "sibling subtask should NOT be collected"
        );
        assert_eq!(collected.len(), 2);
    }

    #[test]
    fn test_collect_transitive_deps_multiple_subtasks_requested() {
        // Test requesting multiple specific subtasks (e.g., install:td install:cs)
        let task_deps = HashMap::new();

        let mut task_specs = HashMap::new();

        let parent_spec = TaskSpec {
            name: "install".to_string(),
            action: String::new(),
            ..Default::default()
        };
        task_specs.insert("install".to_string(), parent_spec);

        let subtask_td = TaskSpec {
            name: "install:td".to_string(),
            action: "echo td".to_string(),
            ..Default::default()
        };
        task_specs.insert("install:td".to_string(), subtask_td);

        let subtask_ts = TaskSpec {
            name: "install:ts".to_string(),
            action: "echo ts".to_string(),
            ..Default::default()
        };
        task_specs.insert("install:ts".to_string(), subtask_ts);

        let subtask_cs = TaskSpec {
            name: "install:cs".to_string(),
            action: "echo cs".to_string(),
            ..Default::default()
        };
        task_specs.insert("install:cs".to_string(), subtask_cs);

        let mut collected = HashSet::new();

        // Collect install:td
        Parser::collect_transitive_deps("install:td", &task_deps, &task_specs, &mut collected).unwrap();
        // Collect install:cs
        Parser::collect_transitive_deps("install:cs", &task_deps, &task_specs, &mut collected).unwrap();

        assert!(
            collected.contains("install:td"),
            "first requested subtask should be collected"
        );
        assert!(
            collected.contains("install:cs"),
            "second requested subtask should be collected"
        );
        assert!(
            !collected.contains("install:ts"),
            "unrequested sibling should NOT be collected"
        );
        assert!(!collected.contains("install"), "parent should NOT be collected");
        assert_eq!(collected.len(), 2);
    }

    #[test]
    fn test_collect_transitive_deps_nested_colon_names() {
        // Test task names with multiple colons (e.g., "group:subgroup:item")
        let task_deps = HashMap::new();

        let mut task_specs = HashMap::new();

        // Even with nested colons, contains(':') returns true, so no expansion
        let nested_task = TaskSpec {
            name: "group:sub:item".to_string(),
            action: "echo nested".to_string(),
            ..Default::default()
        };
        task_specs.insert("group:sub:item".to_string(), nested_task);

        // Another nested task that shouldn't be collected
        let other_nested = TaskSpec {
            name: "group:sub:other".to_string(),
            action: "echo other".to_string(),
            ..Default::default()
        };
        task_specs.insert("group:sub:other".to_string(), other_nested);

        let mut collected = HashSet::new();

        Parser::collect_transitive_deps("group:sub:item", &task_deps, &task_specs, &mut collected).unwrap();

        assert!(collected.contains("group:sub:item"));
        assert!(
            !collected.contains("group:sub:other"),
            "nested sibling should NOT be collected"
        );
        assert_eq!(collected.len(), 1);
    }

    #[test]
    fn test_collect_transitive_deps_subtask_with_after() {
        // Test that subtasks can use 'after' and it still works correctly
        let task_deps = HashMap::new();

        let mut task_specs = HashMap::new();

        let parent_spec = TaskSpec {
            name: "install".to_string(),
            action: String::new(),
            ..Default::default()
        };
        task_specs.insert("install".to_string(), parent_spec);

        // install:td has an 'after' that should trigger report
        let subtask_td = TaskSpec {
            name: "install:td".to_string(),
            action: "echo td".to_string(),
            after: vec![crate::cfg::edge::EdgeSpec::sugar("report")],
            ..Default::default()
        };
        task_specs.insert("install:td".to_string(), subtask_td);

        let subtask_ts = TaskSpec {
            name: "install:ts".to_string(),
            action: "echo ts".to_string(),
            ..Default::default()
        };
        task_specs.insert("install:ts".to_string(), subtask_ts);

        let report_spec = TaskSpec {
            name: "report".to_string(),
            action: "echo report".to_string(),
            ..Default::default()
        };
        task_specs.insert("report".to_string(), report_spec);

        let mut collected = HashSet::new();

        Parser::collect_transitive_deps("install:td", &task_deps, &task_specs, &mut collected).unwrap();

        assert!(
            collected.contains("install:td"),
            "requested subtask should be collected"
        );
        assert!(collected.contains("report"), "'after' task should be collected");
        assert!(!collected.contains("install:ts"), "sibling should NOT be collected");
        assert_eq!(collected.len(), 2);
    }

    #[test]
    fn test_collect_transitive_deps_dependency_on_specific_subtask() {
        // Test: task 'deploy' depends on a specific subtask 'install:td'
        // Running 'deploy' should collect install:td but NOT other install subtasks
        let mut task_deps = HashMap::new();
        task_deps.insert("deploy".to_string(), vec![TaskEdge::success("install:td")]);

        let mut task_specs = HashMap::new();

        let parent_spec = TaskSpec {
            name: "install".to_string(),
            action: String::new(),
            ..Default::default()
        };
        task_specs.insert("install".to_string(), parent_spec);

        let subtask_td = TaskSpec {
            name: "install:td".to_string(),
            action: "echo td".to_string(),
            ..Default::default()
        };
        task_specs.insert("install:td".to_string(), subtask_td);

        let subtask_ts = TaskSpec {
            name: "install:ts".to_string(),
            action: "echo ts".to_string(),
            ..Default::default()
        };
        task_specs.insert("install:ts".to_string(), subtask_ts);

        let deploy_spec = TaskSpec {
            name: "deploy".to_string(),
            action: "echo deploy".to_string(),
            ..Default::default()
        };
        task_specs.insert("deploy".to_string(), deploy_spec);

        let mut collected = HashSet::new();

        Parser::collect_transitive_deps("deploy", &task_deps, &task_specs, &mut collected).unwrap();

        assert!(collected.contains("deploy"), "requested task should be collected");
        assert!(
            collected.contains("install:td"),
            "dependency subtask should be collected"
        );
        assert!(
            !collected.contains("install:ts"),
            "other subtask should NOT be collected"
        );
        assert!(!collected.contains("install"), "parent should NOT be collected");
        assert_eq!(collected.len(), 2);
    }

    #[test]
    fn test_collect_transitive_deps_regular_task_no_expansion() {
        // Regular tasks (no colons) that have no subtasks should not try to expand
        let task_deps = HashMap::new();

        let mut task_specs = HashMap::new();

        let build_spec = TaskSpec {
            name: "build".to_string(),
            action: "echo build".to_string(),
            ..Default::default()
        };
        task_specs.insert("build".to_string(), build_spec);

        let test_spec = TaskSpec {
            name: "test".to_string(),
            action: "echo test".to_string(),
            ..Default::default()
        };
        task_specs.insert("test".to_string(), test_spec);

        let mut collected = HashSet::new();

        Parser::collect_transitive_deps("build", &task_deps, &task_specs, &mut collected).unwrap();

        assert!(collected.contains("build"));
        assert!(!collected.contains("test"));
        assert_eq!(collected.len(), 1);
    }

    #[test]
    fn test_subtask_targeting_integration() {
        // Integration test: parse an ottofile with foreach and request specific subtask
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ottofile_path = temp_dir.path().join("otto.yml");

        let config = r#"
tasks:
  install:
    foreach:
      items: [td, ts, cs]
      as: pkg
    bash: echo "Installing ${pkg}"
"#;
        fs::write(&ottofile_path, config).unwrap();

        let args = vec![
            "otto".to_string(),
            "--ottofile".to_string(),
            ottofile_path.to_string_lossy().to_string(),
            "install:td".to_string(), // Request ONLY this subtask
        ];

        let mut parser = Parser::new(args).unwrap();
        let result = parser.parse().unwrap();
        let (tasks, _, _, _, _, _) = result;

        // Should only have install:td, NOT install:ts or install:cs
        let task_names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert!(
            task_names.contains(&"install:td"),
            "requested subtask should be present"
        );
        assert!(
            !task_names.contains(&"install:ts"),
            "sibling subtask should NOT be present"
        );
        assert!(
            !task_names.contains(&"install:cs"),
            "sibling subtask should NOT be present"
        );
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn test_parent_task_runs_all_subtasks_integration() {
        // Integration test: requesting parent task should run all subtasks
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ottofile_path = temp_dir.path().join("otto.yml");

        let config = r#"
tasks:
  install:
    foreach:
      items: [td, ts, cs]
      as: pkg
    bash: echo "Installing ${pkg}"
"#;
        fs::write(&ottofile_path, config).unwrap();

        let args = vec![
            "otto".to_string(),
            "--ottofile".to_string(),
            ottofile_path.to_string_lossy().to_string(),
            "install".to_string(), // Request parent
        ];

        let mut parser = Parser::new(args).unwrap();
        let result = parser.parse().unwrap();
        let (tasks, _, _, _, _, _) = result;

        // Should have all subtasks plus the (now executable) virtual parent
        let task_names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert!(task_names.contains(&"install:td"));
        assert!(task_names.contains(&"install:ts"));
        assert!(task_names.contains(&"install:cs"));
        assert!(task_names.contains(&"install"));
        assert_eq!(tasks.len(), 4);
    }

    #[test]
    fn test_dependency_on_subtask_integration() {
        // Integration test: task depending on specific subtask
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ottofile_path = temp_dir.path().join("otto.yml");

        let config = r#"
tasks:
  install:
    foreach:
      items: [td, ts, cs]
      as: pkg
    bash: echo "Installing ${pkg}"

  deploy:
    before: ["install:td"]
    bash: echo "Deploying"
"#;
        fs::write(&ottofile_path, config).unwrap();

        let args = vec![
            "otto".to_string(),
            "--ottofile".to_string(),
            ottofile_path.to_string_lossy().to_string(),
            "deploy".to_string(),
        ];

        let mut parser = Parser::new(args).unwrap();
        let result = parser.parse().unwrap();
        let (tasks, _, _, _, _, _) = result;

        let task_names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert!(task_names.contains(&"deploy"), "deploy should be present");
        assert!(
            task_names.contains(&"install:td"),
            "dependency subtask should be present"
        );
        assert!(
            !task_names.contains(&"install:ts"),
            "other subtask should NOT be present"
        );
        assert!(
            !task_names.contains(&"install:cs"),
            "other subtask should NOT be present"
        );
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn test_unknown_dependency_errors() {
        // Test that referencing an unknown dependency produces an error
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ottofile_path = temp_dir.path().join("otto.yml");

        let config = r#"
tasks:
  build:
    before: ["nonexistent_task"]
    bash: echo "Building"
"#;
        fs::write(&ottofile_path, config).unwrap();

        let args = vec![
            "otto".to_string(),
            "--ottofile".to_string(),
            ottofile_path.to_string_lossy().to_string(),
            "build".to_string(),
        ];

        let mut parser = Parser::new(args).unwrap();
        let result = parser.parse();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("unknown dependency"),
            "Error should mention unknown dependency: {}",
            err
        );
        assert!(
            err.to_string().contains("nonexistent_task"),
            "Error should mention the dependency name: {}",
            err
        );
    }

    #[test]
    fn test_unknown_subtask_dependency_errors() {
        // Test that referencing a typo'd subtask produces an error
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ottofile_path = temp_dir.path().join("otto.yml");

        let config = r#"
tasks:
  install:
    foreach:
      items: [td, ts, cs]
    bash: echo "Installing ${item}"

  deploy:
    before: ["install:tx"]  # Typo: should be "install:td"
    bash: echo "Deploying"
"#;
        fs::write(&ottofile_path, config).unwrap();

        let args = vec![
            "otto".to_string(),
            "--ottofile".to_string(),
            ottofile_path.to_string_lossy().to_string(),
            "deploy".to_string(),
        ];

        let mut parser = Parser::new(args).unwrap();
        let result = parser.parse();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("unknown dependency"),
            "Error should mention unknown dependency: {}",
            err
        );
        assert!(
            err.to_string().contains("install:tx"),
            "Error should mention the typo'd subtask: {}",
            err
        );
    }

    #[test]
    fn test_valid_dependencies_succeed() {
        // Test that valid dependencies don't trigger the error
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ottofile_path = temp_dir.path().join("otto.yml");

        let config = r#"
tasks:
  setup:
    bash: echo "Setting up"

  build:
    before: ["setup"]
    bash: echo "Building"
"#;
        fs::write(&ottofile_path, config).unwrap();

        let args = vec![
            "otto".to_string(),
            "--ottofile".to_string(),
            ottofile_path.to_string_lossy().to_string(),
            "build".to_string(),
        ];

        let mut parser = Parser::new(args).unwrap();
        let result = parser.parse();

        assert!(result.is_ok(), "Valid dependencies should succeed: {:?}", result.err());
    }

    // =========================================================================
    // help drift regression (docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md Phase 1)
    // =========================================================================

    /// The exact `Options:` block otto's global flags must render as, in every
    /// builder. Pinned so a builder that stops calling `global_args()` (or a
    /// change to `global_args()` that isn't propagated) fails loudly instead
    /// of silently dropping flags from `--help` again.
    ///
    /// `{JOBS}` stands in for `-j/--jobs`'s default, which is `DEFAULT_JOBS`
    /// (`num_cpus::get()` on the machine that renders the help text, not a
    /// fixed number). A literal `32` here pinned this test to the developing
    /// machine's core count: green locally, red on any runner with a
    /// different core count (`docs/design/2026-06-10-code-review-remediation.md`
    /// Phase 0). `expected_global_options_help()` below substitutes the real
    /// default at test time so the anti-drift check still holds everywhere
    /// else in the string.
    const EXPECTED_GLOBAL_OPTIONS_HELP_TEMPLATE: &str = "Options:\n  -C, --cwd <DIR>\n          Change to DIR before doing anything\n\n  -o, --ottofile <PATH>\n          path to the ottofile\n          \n          [env: OTTOFILE=]\n          [default: .]\n\n      --list-subtasks\n          List all foreach subtasks and exit\n\n      --tasks\n          Print the machine-readable task list and exit\n\n      --format <FORMAT>\n          Output format for --tasks (yaml or json); default: yaml on a tty, json when piped\n          \n          [possible values: yaml, json]\n\n  -j, --jobs <N>\n          Number of parallel jobs\n          \n          [default: {JOBS}]\n\n  -t, --tui\n          Enable interactive TUI dashboard for task monitoring\n\n      --no-prefix\n          Suppress the [task] prefix on task output\n\n  -h, --help\n          Print help\n\n  -V, --version\n          Print version";

    /// Renders `EXPECTED_GLOBAL_OPTIONS_HELP_TEMPLATE` against this
    /// machine's actual `-j/--jobs` default, so the comparison is exact
    /// everywhere except the one value that is legitimately
    /// machine-dependent.
    fn expected_global_options_help() -> String {
        EXPECTED_GLOBAL_OPTIONS_HELP_TEMPLATE.replace("{JOBS}", &DEFAULT_JOBS)
    }

    /// Extracts the `Options:` section, from the `Options:` heading through
    /// the auto-appended `-V, --version` entry (always the last flag clap
    /// renders). Builders may append more after that (subcommand-derived
    /// `after_help` error text, in `build_help_command_with_error()`'s case)
    /// which is irrelevant to this test and must not pollute the comparison.
    fn options_section(help: &str) -> &str {
        let start = help
            .find("Options:")
            .expect("help output must contain an Options: section");
        let rest = &help[start..];
        let anchor = "Print version";
        let end = rest.find(anchor).expect("help output must contain -V, --version") + anchor.len();
        &rest[..end]
    }

    #[test]
    fn test_help_global_flags_no_drift() {
        // Parser::new() doesn't load the ottofile (that happens in parse()),
        // so build_help_command() sees an empty config_spec and takes its
        // after_help branch here. That only affects content appended after
        // the Options: section, which options_section() strips off below -
        // irrelevant to this test's concern (global flag parity).
        let args = vec!["otto".to_string()];
        let parser = Parser::new(args).unwrap();

        let otto_cmd_help = Parser::otto_command().render_long_help().to_string();
        let help_cmd_help = parser.build_help_command().render_long_help().to_string();
        let help_cmd_error_help = Parser::build_help_command_with_error().render_long_help().to_string();

        let expected = expected_global_options_help();
        assert_eq!(
            options_section(&otto_cmd_help),
            expected,
            "otto_command() global flags drifted from the pinned snapshot"
        );
        assert_eq!(
            options_section(&help_cmd_help),
            expected,
            "build_help_command() global flags drifted from the pinned snapshot"
        );
        assert_eq!(
            options_section(&help_cmd_error_help),
            expected,
            "build_help_command_with_error() global flags drifted from the pinned snapshot"
        );
    }

    /// `expected_global_options_help()` must reflect this machine's actual
    /// `-j/--jobs` default rather than a value baked in at write time - the
    /// exact bug this fix closes. Locks the substitution itself, not just
    /// the drift check that depends on it.
    #[test]
    fn test_expected_global_options_help_substitutes_actual_jobs_default() {
        let expected = expected_global_options_help();
        assert!(
            expected.contains(&format!("[default: {}]", num_cpus::get())),
            "expected help must reflect this machine's num_cpus::get(), got: {expected}"
        );
        assert!(
            !expected.contains("{JOBS}"),
            "template placeholder must be fully substituted"
        );
    }

    // =========================================================================
    // otto.api version gate (design doc 2026-08-29-strict-ottofile-schema)
    // =========================================================================

    /// Write `content` as an ottofile in a fresh temp dir and load it through
    /// the real config path.
    fn load_ottofile(content: &str) -> (tempfile::TempDir, Result<(ConfigSpec, String, Option<PathBuf>)>) {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let ottofile_path = temp_dir.path().join("otto.yml");
        std::fs::write(&ottofile_path, content).unwrap();
        let result = Parser::load_config_from_path(Some(ottofile_path));
        // The TempDir rides along so it outlives the load.
        (temp_dir, result)
    }

    #[test]
    fn test_load_config_rejects_an_unsupported_api_version() {
        let (_dir, result) = load_ottofile("otto:\n  api: 2\ntasks:\n  up:\n    bash: echo hi\n");
        let err = result.expect_err("api: 2 must not load").to_string();
        assert!(err.contains("unsupported api version '2'"), "{err}");
        assert!(err.contains("this otto supports: 1"), "{err}");
        assert!(err.contains("upgrade otto"), "{err}");
    }

    #[test]
    fn test_load_config_accepts_a_supported_and_an_absent_api_version() {
        let (_dir, declared) = load_ottofile("otto:\n  api: 1\ntasks:\n  up:\n    bash: echo hi\n");
        let (config, ..) = declared.expect("api: 1 must load");
        assert_eq!(config.otto.api, "1");
        assert!(config.tasks.contains_key("up"));

        let (_dir, absent) = load_ottofile("tasks:\n  up:\n    bash: echo hi\n");
        let (config, ..) = absent.expect("an absent api: must load");
        assert_eq!(config.otto.api, "1");
        assert!(config.tasks.contains_key("up"));
    }

    /// THE ORDERING ASSERT. The api gate runs BEFORE the typed parse, so a file
    /// that is both too new AND unparseable by this otto reports "upgrade otto"
    /// rather than a complaint about whichever key it could not read. Reverse
    /// the two statements in `load_config_from_path` and this test fails on the
    /// second assert: serde reports `tasks.up.before: invalid type: map,
    /// expected a sequence`, which tells the operator nothing useful.
    #[test]
    fn test_load_config_reports_the_api_error_before_the_parse_error() {
        let (_dir, result) =
            load_ottofile("otto:\n  api: 2\ntasks:\n  up:\n    before:\n      some: map\n    bash: echo hi\n");
        let err = result.expect_err("a file that is too new must not load").to_string();
        assert!(
            err.contains("unsupported api version '2'"),
            "the version error wins: {err}"
        );
        assert!(
            !err.contains("invalid type"),
            "the parse error must not win over the version error: {err}"
        );
    }
}
