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

// #[derive(Debug, PartialEq, Eq)]
// pub struct OttoDefaults {
//     pub name: String,
//     pub about: String,
//     pub api: String,
//     pub verbosity: String,
//     pub jobs: String,
//     pub tasks: Vec<String>,
// }

// impl Default for OttoDefaults {
//     fn default() -> Self {
//         Self {
//             name: "otto".to_string(),
//             about: "otto".to_string(),
//             api: "".to_string(),
//             verbosity: "1".to_string(),
//             jobs: "1".to_string(),
//             tasks: vec![],
//         }
//     }
// }

// impl From<Otto> for OttoDefaults {
//     fn from(defaults: Otto) -> Self {
//         Self {
//             name: defaults.name.clone(),
//             about: defaults.about.clone(),
//             api: defaults.api.clone(),
//             verbosity: defaults.verbosity.clone(),
//             jobs: defaults.jobs.clone(),
//             tasks: defaults.tasks.clone(),
//         }
//     }
// }

/*
- name: hello
  deps: []
  envs:
    bob: sue
    age: 11
  args:
    greeting: hello
  action: |
    #!/bin/bash
    echo "hello"
- name: world
  deps: [hello]
  envs:
    bob: ann
    age: 13
  args:
    name: world
 */
// #[derive(Debug, PartialEq, Eq)]
// pub struct Task {
//     name: String,
//     deps: Vec<String>,
//     envs: HashMap<String, String>,
//     args: HashMap<String, String>,
//     action: String,
// }

// impl Task {
//     pub fn new(name: String) -> Self {
//         Self {
//             name,
//             deps: vec![],
//             envs: HashMap::new(),
//             args: HashMap::new(),
//             action: "".to_string(),
//         }
//     }
// }

// type Tasks = Vec<Task>;

// impl Default for Task {
//     fn default() -> Self {
//         Self {
//             name: "".to_string(),
//             deps: vec![],
//             envs: HashMap::new(),
//             args: HashMap::new(),
//             action: "".to_string(),
//         }
//     }
// }

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

    /*
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
        for index in Self::indices(args.clone(), task_names).iter().rev() {
            partitions.insert(0, args[*index..end].to_vec());
            end = *index;
        }
        partitions
    }
    */
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

    fn otto_to_command(otto: Otto, tasks: Vec<Task>) -> Command {
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
        for task in tasks {
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

    // fn matches_to_otto_defaults(matches: &ArgMatches) -> OttoDefaults {
    //     let mut otto = OttoDefaults::default();
    //     //println!("{:#?}", matches);
    //     if let Some(verbosity) = matches.get_one::<String>("verbosity") {
    //         otto.verbosity = verbosity.to_owned();
    //     }
    //     if let Some(api) = matches.get_one::<String>("api") {
    //         otto.api = api.to_owned();
    //     }
    //     if let Some(jobs) = matches.get_one::<String>("jobs") {
    //         otto.jobs = jobs.to_owned();
    //     }
    //     if let Some(tasks) = matches.get_many::<String>("tasks") {
    //         let tasks: Vec<String> = tasks.into_iter().map(|t| t.to_owned()).collect();
    //         otto.tasks = tasks.into_iter().map(|t| t.to_owned()).collect();
    //     }
    //     otto
    // }

    // // need to get the task name and envs into the Task2 object
    // fn matches_to_task(matches: &ArgMatches) -> Task {
    //     let mut task = Task::default();
    //     for id in matches.ids().map(|id| id.as_str()) {
    //         if let Some(value) = matches.get_one::<String>(id) {
    //             task.args.insert(id.to_owned(), value.to_owned());
    //         }
    //     }
    //     task
    // }

    // need to get the task name and envs inot the Task object
    fn matches_to_task(task_name: &str, matches: &ArgMatches) -> Task {
        let mut task = Task::default();
        for id in matches.ids().map(|id| id.as_str()) {
            if let Some(value) = matches.get_one::<String>(id) {
                task.values.insert(id.to_owned(), value.to_owned());
            }
        }
        task
    }

    // methods
    // pub fn parse(&self) -> Result<(OttoDefaults, Tasks)> {
    //     let matches_vec = self.get_matches()?;
    //     for matches in matches_vec.iter() {
    //         println!("{:#?}", matches);
    //     }
    //     let (defaults, tasks) = match self.get_matches()?.as_slice() {
    //         [head, tail @ ..] => {
    //             let defaults = Self::matches_to_otto_defaults(head);
    //             let tasks: Vec<Task> = tail
    //                 .iter()
    //                 .map(|matches| Self::matches_to_task(matches))
    //                 .collect();
    //             (defaults, tasks)
    //         },
    //         // we have no matches, throw an error
    //         [] => return Err(eyre!("no matches")),
    //     };
    //     Ok((defaults, tasks))
    // }

    pub fn parse(&self) -> Result<(Otto, Tasks)> {
        let supplied_task_names = self.pargs.iter().map(|p| p[0].clone()).collect();
        let task_matches = self.get_matches2()?;
        for (task_name, matches) in task_matches.iter() {
            println!("task_name: {}", task_name);
            println!("{:#?}", matches);
        }
        let (defaults, tasks) = match task_matches.as_slice() {
            [head, tail @ ..] => {
                //let defaults = Self::matches_to_otto_defaults(head);
                let defaults = self.spec.otto.clone();
                // let tasks: Tasks = tail
                //     .iter()
                //     .map(|(task_name, matches)| Self::matches_to_task(task_name, matches))
                //     .collect();
                let tasks = tail
                    .iter()
                    .map(|(task_name, matches)| {
                        let task = self.spec.tasks.get(task_name).unwrap();
                        let mut task = task.clone();
                        // task.values = matches
                        //     .iter()
                        //     .map(|(k, v)| (k.to_owned(), v.to_owned()))
                        //     .collect();
                        task
                    });
                (defaults, tasks)
            }
            // we have no matches, throw an error
            [] => return Err(eyre!("no matches")),
        };
        Ok((defaults, tasks))
    }

    pub fn get_matches(&self) -> Result<Vec<ArgMatches>> {
        let mut matches_vec = vec![];
        if self.spec.tasks.is_empty() {
            //panic!("no tasks in ottofile");
            // No tasks in ottofile, print base help of otto without subcommands
            let otto = Self::otto_to_command(self.spec.otto.clone(), vec![])
                .disable_help_subcommand(true)
                .arg_required_else_help(true)
                .after_help("after_help");

            let args: Vec<String> = std::iter::once("otto".to_string())
                .chain(self.pargs[0].iter().cloned())
                .collect();

            let matches = otto.get_matches_from(args);
            matches_vec.push(matches);
        } else if self.pargs.len() == 1 {
            //we only have the main otto partition; no tasks
            let otto = Self::otto_to_command(self.spec.otto.clone(), self.spec.tasks.values().cloned().collect())
                .disable_help_subcommand(true)
                .arg_required_else_help(true)
                .after_help("after_help");
            let matches = otto.get_matches_from(&self.pargs[0]);
            matches_vec.push(matches);
        } else if let Some(index) = self
            .pargs
            .iter()
            .position(|p| !p.intersect(vec!["-h".to_owned(), "--help".to_owned()]).is_empty())

        {
            // we have a partition with help
            // build the clap command for the partition with help
            // and the parse the args
            let partition = &self.pargs[index];
            let task = &self.spec.tasks[&partition[0]];
            let command = Self::task_to_command(task)
                .disable_help_subcommand(true)
                .arg_required_else_help(true)
                .after_help("after_help");
            let matches = command.get_matches_from(partition);
            matches_vec.push(matches);
        } else {
            // we don't have a partition with
            // build the clap command for the otto and tasks, respectively
            // and then parse the args (partition) for each task
            let otto = Self::otto_to_command(self.spec.otto.clone(), vec![]).after_help("otto!");
            let otto_matches = otto.get_matches_from(&self.pargs[0]);
            matches_vec.push(otto_matches);
            for partition in &self.pargs[1..] {
                let task = &self.spec.tasks[&partition[0]];
                let command = Self::task_to_command(task).after_help("task!");
                let matches = command.get_matches_from(partition);
                matches_vec.push(matches);
            }
        }
        Ok(matches_vec)
    }

    // return the pair of task name and matches
    pub fn get_matches2(&self) -> Result<Vec<(String, ArgMatches)>> {
        let mut matches_vec = vec![];
        if self.spec.tasks.is_empty() {
            panic!("no tasks in ottofile");
        } else if self.pargs.len() == 1 {
            //we only have the main otto partition; no tasks
            let otto = Self::otto_to_command(self.spec.otto.clone(), self.spec.tasks.values().cloned().collect())
                .disable_help_subcommand(true)
                .arg_required_else_help(true)
                .after_help("after_help");
            let matches = otto.get_matches_from(&self.pargs[0]);
            matches_vec.push(("otto".to_owned(), matches));
        } else if let Some(index) = self
            .pargs
            .iter()
            .position(|p| !p.intersect(vec!["-h".to_owned(), "--help".to_owned()]).is_empty())

        {
            // we have a partition with help
            // build the clap command for the partition with help
            // and the parse the args
            let partition = &self.pargs[index];
            let task = &self.spec.tasks[&partition[0]];
            let command = Self::task_to_command(task)
                .disable_help_subcommand(true)
                .arg_required_else_help(true)
                .after_help("after_help");
            let matches = command.get_matches_from(partition);
            matches_vec.push((partition[0].to_owned(), matches));
        } else {
            // we don't have a partition with
            // build the clap command for the otto and tasks, respectively
            // and then parse the args (partition) for each task
            let otto = Self::otto_to_command(self.spec.otto.clone(), vec![]).after_help("otto!");
            let otto_matches = otto.get_matches_from(&self.pargs[0]);
            matches_vec.push(("otto".to_owned(), otto_matches));
            for partition in &self.pargs[1..] {
                let task = &self.spec.tasks[&partition[0]];
                let command = Self::task_to_command(task).after_help("task!");
                let matches = command.get_matches_from(partition);
                matches_vec.push((partition[0].to_owned(), matches));
            }
        }
        Ok(matches_vec)
    }

    /*
    pub fn get_matches_old(&self) -> Result<Vec<ArgMatches>> {
        let mut matches_vec = vec![];
        if let Some(ottofile) = &self.ottofile {
            //we have an ottofile, so let's load it
            let loader = Loader::new(ottofile);
            let spec = loader.load()?;
            let task_names: Vec<&str> = spec.tasks.keys().map(String::as_str).collect();
            //let partitions = self.partitions(&task_names);
            let partitions = partitions(&self.args, &task_names);
            if task_names.is_empty() {
                // we don't have tasks in the ottofile
                panic!("no tasks in ottofile");
            } else if partitions.len() == 1 {
                //we only have the main otto partition; no tasks
                let otto = Self::otto_to_command(spec.otto.clone(), spec.tasks.values().cloned().collect())
                    .disable_help_subcommand(true)
                    .arg_required_else_help(true)
                    .after_help("after_help");
                let matches = otto.get_matches_from(&partitions[0]);
                matches_vec.push(matches);
            } else if let Some(index) = partitions
                .iter()
                .position(|p| !p.intersect(vec!["-h".to_owned(), "--help".to_owned()]).is_empty())
            {
                // we have a partition with help
                // build the clap command for the partition with help
                // and the parse the args
                let partition = &partitions[index];
                let task = &spec.tasks[&partition[0]];
                let command = Self::task_to_command(task)
                    .disable_help_subcommand(true)
                    .arg_required_else_help(true)
                    .after_help("after_help");
                let matches = command.get_matches_from(partition);
                matches_vec.push(matches);
            } else {
                // we don't have a partition with
                // build the clap command for the otto and tasks, respectively
                // and then parse the args (partition) for each task
                let otto = Self::otto_to_command(spec.otto.clone(), vec![]).after_help("otto!");
                let otto_matches = otto.get_matches_from(&partitions[0]);
                matches_vec.push(otto_matches);
                for partition in &partitions[1..] {
                    let task = &spec.tasks[&partition[0]];
                    let command = Self::task_to_command(task).after_help("task!");
                    let matches = command.get_matches_from(partition);
                    matches_vec.push(matches);
                }
            }
        } else {
            // if we don't have an ottofile
            // force the help message
            let dash = "\n- ";
            let after_help = format!(
                "--ottofile arg not specified, nor OTTOFILE env var, not one of 'OTTOFILES' discovered in path={0}\nOTTOFILES: {1}",
                self.cwd.display(),
                format_items(OTTOFILES, Some(dash), Some(dash), None)
            );
            let otto = Self::otto_to_command(Otto::default(), vec![])
                .disable_help_subcommand(true)
                .arg_required_else_help(true)
                .after_help(after_help);
            let matches = otto.get_matches_from(vec!["--help"]);
            matches_vec.push(matches);
        }
        Ok(matches_vec)
    }
    */
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_format_items() {
        let items = vec!["a", "b", "c"];
        let expect = "?a|b|c!";
        let actual = format_items(&items, Some("?"), Some("|"), Some("!"));
        assert_eq!(expect, actual);
    }
}