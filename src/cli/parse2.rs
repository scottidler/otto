#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use clap::{arg, Arg, ArgMatches, Command};
use eyre::{eyre, Result};
use thiserror::Error;

use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fmt::{Debug, Display, Formatter};
use std::fs;
use std::fs::metadata;
use std::marker::PhantomData;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::unimplemented;

use array_tool::vec::Intersect;
use expanduser::expanduser;

use crate::cfg::load::Loader;
use crate::cfg::spec::{Otto, Param, ParamType, Config, Task, Tasks, Value};
use crate::cli::error::{OttoParseError, OttofileError};

#[macro_use]
use super::macros;
use std::error;

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
pub struct Parser2 {
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

impl Parser2 {
    pub fn new() -> Result<Self> {
        let prog = std::env::current_exe()?
            .file_name()
            .and_then(OsStr::to_str)
            .map_or_else(|| "otto".to_string(), std::string::ToString::to_string);
        let cwd = env::current_dir()?;
        let user = env::var("USER")?;
        let spec = Self::load_spec()?;
        let task_names: Vec<&str> = spec.tasks.keys().map(|k| k.as_str()).collect();
        let args = std::env::args().collect::<Vec<String>>();
        let pargs = partitions(&args, &task_names);
        Ok(Self {
            prog,
            cwd,
            user,
            spec,
            args,
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

    fn get_ottofile_args() -> Result<(Option<PathBuf>, Vec<String>)> {
        let mut args: Vec<String> = env::args().collect();
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
        let ottofile = Self::divine_ottofile(value)?;
        Ok((ottofile, args))
    }

    fn load_spec() -> Result<Config> {
        let mut args: Vec<String> = env::args().collect();
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
            let content = fs::read_to_string(&ottofile)?;
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
        for (name, task) in tasks {
            command = command.subcommand(Self::task_to_command(&task));
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
        let task_commands: Vec<Command> = self.spec.tasks.values().map(Self::task_to_command).collect();

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
        let matches = task_command.try_get_matches_from(args).unwrap();

        // Update the task fields with the parsed values
        for (name, param) in &mut task.params {
            if let Some(value) = matches.get_many::<String>(name) {
                let value_str = value.cloned().next().unwrap_or_default();
                param.value = Value::Item(value_str);
            }
        }
    }

    /*
    // fn update_task_with_args(&self, task: &mut Task, args: &[String]) {
    //     // Update the task fields with the passed parameters from args

    //     // Create a clap command for the given task
    //     let task_command = self.create_task_command(task);

    //     // Parse args using the task command
    //     let matches = task_command.get_matches_from(args);

    //     // Update the task fields with the parsed values
    //     for (name, param) in &mut task.params {
    //         match matches.

    //     }
    // }

    // fn create_task_command(&self, task: &Task) -> Command {
    //     let mut task_command = Command::new(&task.name);

    //     for (name, param) in &task.params {
    //         let mut arg = Arg::new(name);

    //         if let Some(short) = param.short {
    //             arg = arg.short(short);
    //         }

    //         if let Some(long) = &param.long {
    //             arg = arg.long(long);
    //         }

    //         if let Some(help) = &param.help {
    //             arg = arg.help(help);
    //         }

    //         if let Some(metavar) = &param.metavar {
    //             arg = arg.value_name(metavar);
    //         }

    //         task_command = task_command.arg(arg);
    //     }

    //     task_command
    // }

    // fn update_task_with_args(&self, task: &mut Task, args: &[String]) {
    //     // Create a clap command for the given task
    //     let task_command = self.create_task_command(task);

    //     // Parse args using the task command
    //     let matches = task_command.try_get_matches_from(args).unwrap();

    //     // Update the task fields with the parsed values
    //     for (name, param) in &mut task.params {
    //         if let Some(value) = matches.value_of(name) {
    //             param.value = Value::String(value.to_string());
    //         }
    //     }
    // }

    // fn create_task_command(&self, task: &Task) -> Command {
    //     let mut task_command = Command::new(&task.name);

    //     for (name, param) in &task.params {
    //         let mut arg = Arg::new(name);

    //         if let Some(short) = param.short {
    //             arg = arg.short(short);
    //         }

    //         if let Some(long) = &param.long {
    //             arg = arg.long(long);
    //         }

    //         if let Some(help) = &param.help {
    //             arg = arg.help(help);
    //         }

    //         if let Some(metavar) = &param.metavar {
    //             arg = arg.value_name(metavar);
    //         }

    //         task_command = task_command.arg(arg);
    //     }

    //     task_command
    // }

    // fn update_task_with_args(&self, task: &mut Task, args: &[String]) {
    //     // Create a clap command for the given task
    //     let task_command = self.create_task_command(task);

    //     // Parse args using the task command
    //     let matches = task_command.try_get_matches_from(args).unwrap();

    //     // Update the task fields with the parsed values
    //     for (name, param) in &mut task.params {
    //         if let Some(value) = matches.get_many::<String>(name) {
    //             let value_str = value.cloned().next().unwrap_or_default();
    //             param.value = value_str;
    //         }
    //     }
    // }

    // fn update_task_with_args(&self, task: &mut Task, args: &[String]) {
    //     // Create a clap command for the given task
    //     let task_command = self.create_task_command(task);

    //     // Parse args using the task command
    //     let matches = task_command.try_get_matches_from(args).unwrap();

    //     // Update the task fields with the parsed values
    //     for (name, param) in &mut task.params {
    //         if let Some(value) = matches.get_many::<String>(name) {
    //             let value_str = value.cloned().next().unwrap_or_default();
    //             param.value = Value::Item(value_str); // Wrap the value_str with the appropriate enum variant
    //         }
    //     }
    // }
    */

    fn handle_no_input(&self) -> Result<(Otto, Vec<Task>)> {
        // Show default help for 'otto' command and exit
        unimplemented!()
    }

    fn parse_otto_command(&self, otto_command: Command, args: &[String]) -> Result<Otto> {
        // Parse 'otto' command using args and update the Otto fields
        unimplemented!()
    }

    fn parse_task_commands(&self, task_commands: &[Command], args: &[Vec<String>]) -> Result<HashMap<String, Task>> {
        // Parse task commands using args and update the Task fields
        unimplemented!()
    }

    fn parse_and_update_task_fields(&self, tasks: &mut HashMap<String, Task>, args: &[Vec<String>]) {
        // Parse and update the Task fields for tasks using args
        unimplemented!()
    }
}
