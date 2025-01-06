//#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fmt::Debug;
use std::fs;
use std::path::{Path, PathBuf};

use clap::{value_parser, Arg, Command};
use daggy::{Dag, NodeIndex};
use expanduser::expanduser;
use eyre::{eyre, Result};
use hex;
use sha2::{Digest, Sha256};
use log::{debug, warn};

use crate::cfg::config::{Config, Otto, Param, Task, Tasks, Value};

pub type DAG<T> = Dag<T, (), u32>;

const OTTOFILES: &[&str] = &[
    "otto.yml",
    ".otto.yml",
    "otto.yaml",
    ".otto.yaml",
    "Ottofile",
    "OTTOFILE",
];

const DEFAULT_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn calculate_hash(action: &String) -> String {
    let mut hasher = Sha256::new();
    hasher.update(action);
    let result = hasher.finalize();
    hex::encode(result)
}

#[allow(dead_code)]
fn print_type_of<T>(t: &T)
where
    T: ?Sized + Debug,
{
    println!("type={} value={:#?}", std::any::type_name::<T>(), t);
}

#[allow(dead_code)]
fn format_items(items: &[&str], before: Option<&str>, between: Option<&str>, after: Option<&str>) -> String
where
{
    //if between is not None, then join with between
    //if between is None, then join with ""
    let mut s = between.map_or_else(|| items.join(""), |between| items.join(between));
    //if before is not None, then prepend with before
    if let Some(before) = before {
        s = format!("{before}{s}");
    }
    //if after is not None, then append with after
    if let Some(after) = after {
        s = format!("{s}{after}");
    }
    s
}

// This routine is adapted from the *old* Path's `path_relative_from`
// function, which works differently from the new `relative_from` function.
// In particular, this handles the case on unix where both paths are
// absolute but with only the root as the common directory.
// url: https://stackoverflow.com/a/39343127
#[allow(clippy::similar_names)]
fn path_relative_from(path: &Path, base: &Path) -> Option<PathBuf> {
    use std::path::Component;

    if path.is_absolute() == base.is_absolute() {
        let mut ita = path.components();
        let mut itb = base.components();
        let mut comps: Vec<Component> = vec![];
        loop {
            match (ita.next(), itb.next()) {
                (None, None) => break,
                (Some(a), None) => {
                    comps.push(a);
                    comps.extend(ita.by_ref());
                    break;
                }
                (None, _) => comps.push(Component::ParentDir),
                (Some(a), Some(b)) if comps.is_empty() && a == b => (),
                (Some(a), Some(b)) if b == Component::CurDir => comps.push(a),
                (Some(_), Some(b)) if b == Component::ParentDir => return None,
                (Some(a), Some(_)) => {
                    comps.push(Component::ParentDir);
                    for _ in itb {
                        comps.push(Component::ParentDir);
                    }
                    comps.push(a);
                    comps.extend(ita.by_ref());
                    break;
                }
            }
        }
        let val: PathBuf = comps.iter().map(|c| c.as_os_str()).collect();
        if val == Path::new("") {
            Some(PathBuf::from(path))
        } else {
            Some(comps.iter().map(|c| c.as_os_str()).collect())
        }
    } else if path.is_absolute() {
        Some(PathBuf::from(path))
    } else {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskSpec {
    pub name: String,
    pub deps: Vec<String>,
    pub envs: HashMap<String, String>,
    pub values: HashMap<String, Value>,
    pub action: String,
    pub hash: String,
}

impl TaskSpec {
    #[must_use]
    pub fn new(
        name: String,
        deps: Vec<String>,
        envs: HashMap<String, String>,
        values: HashMap<String, Value>,
        action: String,
    ) -> Self {
        let hash = calculate_hash(&action);
        Self {
            name,
            deps,
            envs,
            values,
            action,
            hash,
        }
    }
    #[must_use]
    pub fn from_task(task: &Task) -> Self {
        let name = task.name.clone();
        let deps = task.before.clone();
        let envs = HashMap::new();
        let values = HashMap::new();
        let action = task.action.clone();
        Self::new(name, deps, envs, values, action)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Parser {
    prog: String,
    cwd: PathBuf,
    user: String,
    config: Config,
    hash: String,
    args: Vec<String>,
    pargs: Vec<Vec<String>>,
}

fn indices(args: &[String], task_names: &[&str]) -> Vec<usize> {
    let mut indices = vec![0];
    for (i, arg) in args.iter().enumerate() {
        if task_names.contains(&arg.as_str()) {
            indices.push(i);
        }
    }
    indices
}

fn partitions(args: &Vec<String>, task_names: &[&str]) -> Vec<Vec<String>> {
    let mut partitions = vec![];
    let mut end = args.len();
    for index in indices(args, task_names).iter().rev() {
        partitions.insert(0, args[*index..end].to_vec());
        end = *index;
    }
    partitions
}

impl Parser {
    pub fn new(args: Vec<String>) -> Result<Self> {
        let mut args = args;
        let prog = std::env::current_exe()?
            .file_name()
            .and_then(OsStr::to_str)
            .map_or_else(|| "otto".to_string(), std::string::ToString::to_string);
        let cwd = env::current_dir()?;
        let user = env::var("USER")?;
        let (config, hash) = Self::load_config(&mut args)?;
        let task_names: Vec<&str> = config.tasks.keys().map(std::string::String::as_str).collect();
        let pargs = partitions(&args, &task_names);
        Ok(Self {
            prog,
            cwd,
            user,
            config,
            hash,
            args,
            pargs,
        })
    }

    fn find_ottofile(path: &Path) -> Result<Option<PathBuf>> {
        let cwd = env::current_dir()?;
        debug!("Searching for Ottofile in directory: {:?}", path);
        for ottofile in OTTOFILES {
            let ottofile_path = path.join(ottofile);
            debug!("Checking for file: {:?}", ottofile_path);
            if ottofile_path.exists() {
                debug!("Found Ottofile: {:?}", ottofile_path);
                let relative_path = path_relative_from(&ottofile_path, &cwd)
                    .ok_or_else(|| eyre!("Could not determine relative path"))?;
                return Ok(Some(relative_path));
            }
        }
        if let Some(parent) = path.parent() {
            if parent == Path::new("/") {
                debug!("Reached root directory; Ottofile not found.");
                return Ok(None);
            }
            return Self::find_ottofile(parent);
        }
        debug!("No parent directory; Ottofile not found.");
        Ok(None)
    }

    fn divine_ottofile(value: String) -> Result<Option<PathBuf>> {
        debug!("Divining Ottofile from value: {:?}", value);

        // Expand `~` and canonicalize the path
        let path = expanduser(value)?;
        let canonical_path = fs::canonicalize(&path)?;
        debug!("Canonicalized path: {:?}", canonical_path);

        if canonical_path.is_file() {
            // If the path is a file, return it directly
            debug!("Path is a file; using it directly: {:?}", canonical_path);
            return Ok(Some(canonical_path));
        }

        if canonical_path.is_dir() {
            // If the path is a directory, search for an Ottofile inside
            debug!("Path is a directory; searching for Ottofile inside.");
            return Self::find_ottofile(&canonical_path);
        }

        // If neither, return None
        warn!("Path is neither a valid file nor a directory: {:?}", canonical_path);
        Ok(None)
    }

    fn load_config(args: &mut Vec<String>) -> Result<(Config, String)> {
        debug!("Loading configuration with args: {:?}", args);

        // Check if the --ottofile flag is present
        let index = args.iter().position(|x| x == "--ottofile" || x == "-o");
        let value = index.map_or_else(
            || {
                let env_var = env::var("OTTOFILE").unwrap_or_else(|_| "./".to_owned());
                debug!("No --ottofile flag; using OTTOFILE environment variable or default: {:?}", env_var);
                env_var
            },
            |index| {
                let value = args[index + 1].clone();
                debug!("Found --ottofile flag with value: {:?}", value);
                args.remove(index); // Remove the flag
                args.remove(index); // Remove the value
                value
            },
        );

        // Resolve the Ottofile path using divine_ottofile
        if let Some(ottofile) = Self::divine_ottofile(value.clone())? {
            debug!("Resolved Ottofile path: {:?}", ottofile);

            // Ensure the resolved path is a file
            if !ottofile.is_file() {
                return Err(eyre!("Resolved Ottofile is not a valid file: {:?}", ottofile));
            }

            // Load the Ottofile content
            let content = fs::read_to_string(&ottofile)?;
            debug!("Ottofile content loaded successfully.");
            let hash = calculate_hash(&content);
            debug!("Calculated hash for Ottofile: {:?}", hash);

            // Parse the content into a Config struct
            let config: Config = serde_yaml::from_str(&content)?;
            debug!("Parsed Ottofile content into config.");
            return Ok((config, hash));
        }

        warn!("Ottofile not found; returning default configuration.");
        Ok((Config::default(), DEFAULT_HASH.to_owned()))
    }

    fn otto_to_command(otto: &Otto, tasks: &Tasks) -> Command {
        let mut command = Command::new(&otto.name)
            .bin_name(&otto.name)
            .about(&otto.about)
            .arg(
                Arg::new("ottofile")
                    .short('o')
                    .long("ottofile")
                    //.takes_value(true)
                    .value_name("PATH")
                    .default_value("./")
                    .help("path to the ottofile"),
            )
            .arg(
                Arg::new("api")
                    .short('a')
                    .long("api")
                    //.takes_value(true)
                    .value_name("URL")
                    .default_value(&otto.api)
                    .help("api url"),
            )
            .arg(
                Arg::new("jobs")
                    .short('j')
                    .long("jobs")
                    .value_name("JOBS")
                    .default_value(&otto.jobs.to_string())
                    .value_parser(value_parser!(usize))
                    .help("number of jobs to run in parallel"),
            )
            .arg(
                Arg::new("home")
                    .short('H')
                    .long("home")
                    .value_name("PATH")
                    .default_value(&otto.home)
                    .help("path to the Otto home directory"),
            )
            .arg(
                Arg::new("tasks")
                    .short('t')
                    .long("tasks")
                    //.takes_value(true)
                    .value_name("TASKS")
                    .default_values(&otto.tasks)
                    .help("comma separated list of tasks to run"),
            )
            .arg(
                Arg::new("verbosity")
                    .short('v')
                    .long("verbosity")
                    //.takes_value(true)
                    .value_name("LEVEL")
                    .default_value("1")
                    .help("verbosity level"),
            );
        for task in tasks.values() {
            command = command.subcommand(Self::task_to_command(task));
        }
        command
    }

    fn task_to_command(task: &Task) -> Command {
        let mut command = Command::new(&task.name).bin_name(&task.name);
        if let Some(task_help) = &task.help {
            command = command.about(task_help);
        }
        for param in task.params.values() {
            command = command.arg(Self::param_to_arg(param));
        }
        command
    }

    fn param_to_arg(param: &Param) -> Arg {
        let mut arg = Arg::new(&param.name);
        if let Some(short) = param.short {
            arg = arg.short(short);
        }
        if let Some(long) = &param.long {
            arg = arg.long(long);
        }
        // if param.param_type == ParamType::OPT {
        //     arg = arg.takes_value(true);
        // }
        if let Some(help) = &param.help {
            arg = arg.help(help);
        }
        if let Some(default) = &param.default {
            arg = arg.default_value(default);
        }
        arg
    }

    pub fn parse(&mut self) -> Result<(Otto, DAG<TaskSpec>, String)> {
        // Create clap commands for 'otto' and jobs
        let otto_command = Self::otto_to_command(&self.config.otto, &self.config.tasks);

        // Parse 'otto' command and update Otto fields
        let mut otto = self.parse_otto_command(otto_command, &self.pargs[0])?;

        // Process the jobs with their default values and command line parameters
        let tasks = self.process_tasks()?;

        // Collect the first item in each parg, skipping the first one.
        let configured_tasks = self.pargs.iter().skip(1).map(|p| p[0].clone()).collect::<Vec<String>>();

        // If tasks were passed as arguments, they replace the default tasks.
        // Otherwise, the default tasks remain.
        if configured_tasks.is_empty() {
            otto.tasks = self.config.otto.tasks.clone();
        } else {
            otto.tasks = configured_tasks;
        }

        // Return all jobs from the Ottofile, and the updated Otto struct
        Ok((otto, tasks, self.hash.clone()))
    }

    fn process_tasks(&self) -> Result<DAG<TaskSpec>> {
        // Initialize an empty Dag and an index map
        let mut dag: DAG<TaskSpec> = DAG::new();
        let mut indices: HashMap<String, NodeIndex<u32>> = HashMap::new();

        // Iterate through the tasks loaded from the Ottofile
        for task in self.config.tasks.values() {
            // Create a new job based on the task
            let mut spec = TaskSpec::from_task(task);

            // Apply the default values for each task
            for (name, param) in &task.params {
                if let Some(default_value) = &param.default {
                    let value = Value::Item(default_value.clone());
                    spec.values.insert(name.clone(), value);
                }
            }

            // Check if the task is mentioned in the command line and override the default values with the passed parameters
            if let Some(task_args) = self.pargs[1..].iter().find(|partition| partition[0] == task.name) {
                // Create a clap command for the given task
                let task_command = Self::task_to_command(task);

                // Parse args using the task command
                let matches = task_command.get_matches_from(task_args);

                // Update the job fields with the parsed values
                for param in task.params.values() {
                    if let Some(value) = matches.get_one::<String>(param.name.as_str()) {
                        spec.values.insert(param.name.clone(), Value::Item(value.to_string()));
                    }
                }
            }

            // Add the processed job to the Dag and index map
            let index = dag.add_node(spec.clone());
            indices.insert(spec.name.clone(), index);
        }

        // Iterate over the jobs a second time to handle 'after' dependencies
        for task in self.config.tasks.values() {
            for after_task_name in &task.after {
                let node = indices.get(&task.name).expect("Job not found in indices");
                let dep_node = indices.get(after_task_name).expect("Dependency not found in indices");
                dag.add_edge(*dep_node, *node, ())?;
            }
        }

        // Return the completed Dag
        Ok(dag)
    }

    fn handle_no_input(&self) {
        // Create a default otto command with no tasks
        let otto_command = Self::otto_to_command(&self.config.otto, &HashMap::new());
        otto_command.get_matches_from(["otto", "--help"]);
    }

    fn parse_otto_command(&self, otto_command: Command, args: &[String]) -> Result<Otto> {
        // Parse 'otto' command using args and update the Otto fields

        // if config.tasks is empty, then show default help for 'otto' command and exit
        if self.config.tasks.is_empty() {
            self.handle_no_input();
        }

        // Parse the arguments using clap's get_matches_from method
        let matches = otto_command.get_matches_from(args);

        let mut otto = self.config.otto.clone();

        if matches.contains_id("api") {
            if let Some(api) = matches.get_one::<String>("api") {
                otto.api = api.to_string();
            }
        }
        if matches.contains_id("verbosity") {
            if let Some(verbosity) = matches.get_one::<String>("verbosity") {
                otto.verbosity = verbosity.to_string();
            }
        }

        if matches.contains_id("jobs") {
            if let Some(jobs) = matches.get_one::<usize>("jobs") {
                otto.jobs = *jobs;
            }
        }
        if matches.contains_id("tasks") {
            if let Some(tasks) = matches.get_many::<String>("tasks") {
                otto.tasks = tasks
                    .into_iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<String>>();
            }
        }

        // right now args will have at least one element, the name of the otto binary
        // tasks will be ["*"]
        // so this logic is bunk at the moment
        if args.len() == 1 && otto.tasks.is_empty() {
            return Err(eyre!("No tasks configified"));
        }

        Ok(otto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_indices() {
        let args = vec_of_strings!["arg1", "task1", "arg2", "task2", "arg3",];
        let task_names = &["task1", "task2"];
        let expected = vec![0, 1, 3];
        assert_eq!(indices(&args, task_names), expected);
    }

    #[test]
    fn test_partitions() {
        let args = vec_of_strings!["arg1", "task1", "arg2", "task2", "arg3",];
        let task_names = vec!["task1", "task2"];
        assert_eq!(
            partitions(&args, &task_names),
            vec![vec!["arg1"], vec!["task1", "arg2"], vec!["task2", "arg3"]]
        );
    }

    #[test]
    fn test_parser_new() {
        let args = vec![];
        assert!(Parser::new(args).is_ok());
    }

    fn generate_test_otto() -> Otto {
        Otto {
            name: "otto".to_string(),
            home: "~/.otto".to_string(),
            about: "A task runner".to_string(),
            api: "http://localhost:8000".to_string(),
            jobs: num_cpus::get(),
            verbosity: "1".to_string(),
            tasks: vec!["build".to_string()],
        }
    }

    fn generate_test_task() -> Task {
        Task {
            name: "build".to_string(),
            help: Some("Build the project".to_string()),
            params: HashMap::new(),
            before: vec![],
            after: vec![],
            action: "echo 'building'".to_string(),
        }
    }

    #[test]
    fn test_handle_no_input_no_ottofile() {
        let args = vec![];
        let parser = Parser::new(args).unwrap();

        // Rename or delete Ottofile in current directory if it exists
        if Path::new("Ottofile").exists() {
            fs::rename("Ottofile", "Ottofile.bak").unwrap();
        }

        // Call handle_no_input and check that it doesn't panic
        let result = std::panic::catch_unwind(|| parser.handle_no_input());
        assert!(result.is_ok(), "handle_no_input panicked when no Ottofile was present");

        // Restore Ottofile
        if Path::new("Ottofile.bak").exists() {
            fs::rename("Ottofile.bak", "Ottofile").unwrap();
        }
    }

    #[test]
    fn test_parse_no_args() {
        let otto = generate_test_otto();
        println!("generated otto: {otto:#?}");

        let args = vec!["otto".to_string()];
        let pargs = partitions(&args, &["build"]);

        let mut parser = Parser {
            hash: DEFAULT_HASH.to_string(),
            prog: "otto".to_string(),
            cwd: env::current_dir().unwrap(),
            user: env::var("USER").unwrap(),
            config: Config {
                otto,
                tasks: HashMap::new(),
            },
            args,
            pargs,
        };

        let result = parser.parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_with_args() {
        let otto = generate_test_otto();
        let task = generate_test_task();

        let mut tasks = HashMap::new();
        tasks.insert(task.name.clone(), task);

        let args = vec!["otto".to_string(), "build".to_string()];
        let pargs = partitions(&args, &["build"]);

        let mut parser = Parser {
            prog: "otto".to_string(),
            hash: DEFAULT_HASH.to_string(),
            cwd: env::current_dir().unwrap(),
            user: env::var("USER").unwrap(),
            config: Config {
                otto: otto.clone(),
                tasks: tasks.clone(),
            },
            args,
            pargs,
        };

        let result = parser.parse().unwrap();
        let dag = result.1;

        assert_eq!(result.0, otto, "comparing otto struct");

        // We expect the same number of jobs as tasks
        assert_eq!(dag.node_count(), tasks.len(), "comparing tasks length");

        // Use node_weight to get Job data
        let first_node_index = NodeIndex::new(0);
        let first_task = dag.node_weight(first_node_index).unwrap();

        // Assert job name
        assert_eq!(first_task.name, "build".to_string(), "comparing task name");
    }
}
