//#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use clap::{Arg, Command};
use eyre::{eyre, Result};

use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fmt::Debug;
use std::fs;
use std::path::{Path, PathBuf};
use expanduser::expanduser;

use crate::cfg::spec::{Otto, Param, Config, Task, Tasks, Value};
use crate::cli::error::SilentError;

const OTTOFILES: &[&str] = &[
    "otto.yml",
    ".otto.yml",
    "otto.yaml",
    ".otto.yaml",
    "Ottofile",
    "OTTOFILE",
];

fn print_type_of<T>(t: &T)
where
    T: ?Sized + Debug,
{
    println!("type={} value={:#?}", std::any::type_name::<T>(), t);
}

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
#[derive(Debug, PartialEq, Eq)]
pub struct Parser {
    prog: String,
    cwd: PathBuf,
    user: String,
    spec: Config,
    args: Vec<String>,
    pargs: Vec<Vec<String>>,
}

fn indices(args: Vec<String>, task_names: &[&str]) -> Vec<usize> {
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
    for index in indices(args.clone(), task_names).iter().rev() {
        partitions.insert(0, args[*index..end].to_vec());
        end = *index;
    }
    partitions
}

impl Parser {
    pub fn new(args: &mut Vec<String>) -> Result<Self> {
        let prog = std::env::current_exe()?
            .file_name()
            .and_then(OsStr::to_str)
            .map_or_else(|| "otto".to_string(), std::string::ToString::to_string);
        let cwd = env::current_dir()?;
        let user = env::var("USER")?;
        let spec = Self::load_spec(args)?;
        let task_names: Vec<&str> = spec.tasks.keys().map(std::string::String::as_str).collect();
        let pargs = partitions(&args, &task_names);
        Ok(Self {
            prog,
            cwd,
            user,
            spec,
            args: args.to_owned(),
            pargs,
        })
    }

    fn find_ottofile(path: &Path) -> Result<Option<PathBuf>> {
        let cwd = env::current_dir()?;
        for ottofile in OTTOFILES {
            let ottofile_path = path.join(ottofile);
            if ottofile_path.exists() {
                let p =
                    path_relative_from(&ottofile_path, &cwd).ok_or_else(|| eyre!("could not find relative path"))?;
                return Ok(Some(p));
            }
        }
        let Some(parent) = path.parent() else { return Ok(None)};
        if parent == Path::new("/") {
            return Ok(None);
        }
        Self::find_ottofile(parent)
    }

    fn divine_ottofile(value: String) -> Result<Option<PathBuf>> {
        let mut path = expanduser(value)?;
        path = fs::canonicalize(path)?;
        if path.is_dir() {
            return Self::find_ottofile(&path);
        }
        Ok(Some(path))
    }

    fn load_spec(args: &mut Vec<String>) -> Result<Config> {
        //let mut args: Vec<String> = env::args().collect();
        let index = args.iter().position(|x| x == "--ottofile");
        let value = index.map_or_else(
            || env::var("OTTOFILE").unwrap_or_else(|_| "./".to_owned()),
            |index| {
                let value = args[index + 1].clone();
                args.remove(index);
                args.remove(index);
                value
            },
        );
        if let Some(ottofile) = Self::divine_ottofile(value)? {
            let content = fs::read_to_string(ottofile)?;
            let spec: Config = serde_yaml::from_str(&content)?;
            Ok(spec)
        } else {
            Ok(Config::default())
        }
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
                Arg::new("verbosity")
                    .short('v')
                    .long("verbosity")
                    //.takes_value(true)
                    .value_name("LEVEL")
                    .default_value("1")
                    .help("verbosity level"),
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
                    //.takes_value(true)
                    .value_name("JOBS")
                    .default_value(&otto.jobs.to_string())
                    .help("number of jobs to run in parallel"),
            )
            .arg(
                Arg::new("tasks")
                    .short('t')
                    .long("tasks")
                    //.takes_value(true)
                    .value_name("TASKS")
                    .default_values(&otto.tasks)
                    .help("comma separated list of tasks to run"),
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

    pub fn parse(&mut self) -> Result<(Otto, Vec<Task>)> {
        // Create clap commands for 'otto' and tasks
        let otto_command = Self::otto_to_command(&self.spec.otto, &self.spec.tasks);
        //let task_commands: Vec<Command> = self.spec.tasks.values().map(Self::task_to_command).collect();

        // Parse 'otto' command and update Otto fields
        let mut otto = self.parse_otto_command(otto_command, &self.pargs[0])?;

        // Process the tasks with their default values and command line parameters
        let tasks: Vec<Task> = self.process_tasks()?;

        let mut specified_tasks = Vec::new();

        // Iterate through the command line arguments partitions
        for partition in &self.pargs[1..] {
            let task_name = &partition[0];
            if let Some(task) = tasks.iter().find(|t| t.name == *task_name) {
                specified_tasks.push(task.clone());
            }
        }

        // If no tasks are specified in the command line arguments, use Otto's default tasks
        if specified_tasks.is_empty() {
            otto.tasks = tasks.iter().map(|task| task.name.clone()).collect();
        } else {
            // Update Otto's tasks with the tasks specified in the command line arguments
            otto.tasks = specified_tasks.iter().map(|task| task.name.clone()).collect();
        }

        // Return all tasks from the Ottofile, and the updated Otto struct
        Ok((otto, tasks))
    }

    fn process_tasks(&self) -> Result<Vec<Task>> {
        // Initialize an empty tasks vector
        let mut tasks = vec![];

        // Iterate through the tasks loaded from the Ottofile
        for task in self.spec.tasks.values() {
            // Create a new task with the same fields as the original task
            let mut processed_task = task.clone();

            // Apply the default values for each task
            for (name, param) in &task.params {
                if let Some(default_value) = &param.default {
                    processed_task.values.insert(name.clone(), default_value.clone());
                }
            }

            // Check if the task is mentioned in the command line and override the default values with the passed parameters
            if let Some(task_args) = self.pargs[1..].iter().find(|partition| partition[0] == task.name) {
                // Update the processed_task with the passed parameters
                self.update_task_with_args(&mut processed_task, task_args);
            }

            // Add the processed task to the tasks vector
            tasks.push(processed_task);
        }

        Ok(tasks)
    }

    fn update_task_with_args(&self, task: &mut Task, args: &[String]) {
        // Create a clap command for the given task
        let task_command = Self::task_to_command(task);

        // Parse args using the task command
        let matches = task_command.get_matches_from(args);

        // Update the task fields with the parsed values
        for param in task.params.values_mut() {
            if let Some(mut value) = matches.get_many::<String>(&param.name) {
                let value_str = value.next().cloned().unwrap_or_default();
                param.value = Value::Item(value_str);
            }
        }
    }

    fn handle_no_input(&self) -> Result<(Otto, Vec<Task>)> {
        // Create a default otto command with no tasks
        let mut otto_command = Self::otto_to_command(&self.spec.otto, &HashMap::new());
        otto_command.print_help()?;
        Err(SilentError.into())
    }

    fn parse_otto_command(&self, otto_command: Command, args: &[String]) -> Result<Otto> {
        // Parse 'otto' command using args and update the Otto fields

        // if spec.tasks is empty, then show default help for 'otto' command and exit
        if self.spec.tasks.is_empty() {
            self.handle_no_input()?;
        }

        // Parse the arguments using clap's get_matches_from method
        let matches = otto_command.get_matches_from(args);

        let mut otto = self.spec.otto.clone();

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
            if let Some(jobs) = matches.get_one::<String>("jobs") {
                otto.jobs = jobs.to_string();
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
            return Err(eyre!("No tasks specified"));
        }
        println!("args: {args:?}");
        println!("otto.tasks: {:?}", otto.tasks);

        Ok(otto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_indices() {
        let args = vec![
            "arg1".to_string(),
            "task1".to_string(),
            "arg2".to_string(),
            "task2".to_string(),
            "arg3".to_string(),
        ];
        let task_names = &["task1", "task2"];
        let expected = vec![0, 1, 3];
        assert_eq!(indices(args, task_names), expected);
    }

    #[test]
    fn test_partitions() {
        let args = vec![
            "arg1".to_string(),
            "task1".to_string(),
            "arg2".to_string(),
            "task2".to_string(),
            "arg3".to_string(),
        ];
        let task_names = vec!["task1", "task2"];
        assert_eq!(
            partitions(&args, &task_names),
            vec![
                vec!["arg1"],
                vec!["task1", "arg2"],
                vec!["task2", "arg3"]
            ]
        );
    }

    #[test]
    fn test_parser_new() {
        let mut args = vec![];
        assert!(Parser::new(&mut args).is_ok());
    }

    fn generate_test_otto() -> Otto {
        Otto {
            verbosity: "1".to_string(),
            name: "otto".to_string(),
            about: "A task runner".to_string(),
            api: "http://localhost:8000".to_string(),
            jobs: "4".to_string(),
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
            values: HashMap::new(),
        }
    }

    #[test]
    fn test_handle_no_input_no_ottofile() {
        let mut args = vec![];
        let parser = Parser::new(&mut args).unwrap();

        // Rename or delete Ottofile in current directory if it exists
        if Path::new("Ottofile").exists() {
            fs::rename("Ottofile", "Ottofile.bak").unwrap();
        }

        let result = parser.handle_no_input();
        assert!(result.is_err(), "Expected error when no Ottofile is present");

        // Restore Ottofile
        if Path::new("Ottofile.bak").exists() {
            fs::rename("Ottofile.bak", "Ottofile").unwrap();
        }
    }

    #[test]
    fn test_parse_no_args() {
        let otto = generate_test_otto();

        let args = vec!["otto".to_string()];
        let pargs = partitions(&args, &["build"]);

        let mut parser = Parser {
            prog: "otto".to_string(),
            cwd: env::current_dir().unwrap(),
            user: env::var("USER").unwrap(),
            spec: Config { otto, tasks: HashMap::new() },
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
            cwd: env::current_dir().unwrap(),
            user: env::var("USER").unwrap(),
            spec: Config { otto: otto.clone(), tasks: tasks.clone() },
            args,
            pargs,
        };

        let result = parser.parse().unwrap();
        assert_eq!(result.0, otto, "comparing otto struct");
        assert_eq!(result.1.len(), tasks.len(), "comparing tasks length");
        assert_eq!(result.1[0].name, "build".to_string(), "comparing task name");
    }

    use super::{Parser, Value};

    #[test]
    fn test_parser_parse() {
        let mut args: Vec<String> = vec![
            "--ottofile",
            "examples/ex1",
            "hello",
            "-g",
            "howdy",
            "world",
            "-n",
            "earth",
        ].iter().map(std::string::ToString::to_string).collect();

        let mut parser = Parser::new(&mut args).expect("Failed to create parser");
        let (otto, tasks) = parser.parse().expect("Failed to parse");

        // Add assertions here to verify the parsing was correct
        // For example, check that the 'hello' task has a '-g' param with value 'howdy'
        let hello_task = tasks.iter().find(|task| task.name == "hello")
            .expect("Task 'hello' not found");
        let hello_param = hello_task.params.get(&"-g|--greeting".to_string())
            .expect("Param '-g|--greeting' not found in 'hello' task");
        assert_eq!(hello_param.value, Value::Item("howdy".to_string()));

        // And check that the 'world' task has a '-n' param with value 'earth'
        let world_task = tasks.iter().find(|task| task.name == "world")
            .expect("Task 'world' not found");
        let world_param = world_task.params.get(&"-n|--name".to_string())
            .expect("Param '-n|--name' not found in 'world' task");
        assert_eq!(world_param.value, Value::Item("earth".to_string()));
    }

}
