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
use std::unimplemented;

use array_tool::vec::Intersect;
use expanduser::expanduser;

use crate::cfg::loader::Loader;
use crate::cfg::param::{IParam, Param, ParamType, Params, Value};
use crate::cfg::spec::{Otto, Spec};
use crate::cfg::task::{ITask, Task, Tasks};
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

fn print_type_of<T: ?Sized>(t: &T)
where
    T: Debug,
{
    println!("type={} value={:#?}", std::any::type_name::<T>(), t);
}

fn format_items(items: &[&str], before: Option<&str>, between: Option<&str>, after: Option<&str>) -> String
where
{
    //if between is not None, then join with between
    //if between is None, then join with ""
    let mut s = match between {
        Some(between) => items.join(between),
        None => items.join(""),
    };
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

    if path.is_absolute() != base.is_absolute() {
        if path.is_absolute() {
            Some(PathBuf::from(path))
        } else {
            None
        }
    } else {
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
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Parser {
    prog: String,
    cwd: PathBuf,
    user: String,
    ottofile: Option<PathBuf>,
    args: Vec<String>,
}

impl Parser {
    pub fn new() -> Result<Self> {
        let prog = std::env::current_exe()?
            .file_name()
            .and_then(OsStr::to_str)
            .map_or_else(|| "otto".to_string(), std::string::ToString::to_string);
        let cwd = env::current_dir()?;
        let user = env::var("USER")?;
        let (ottofile, args) = Self::get_ottofile_args()?;
        Ok(Self {
            prog,
            cwd,
            user,
            ottofile,
            args,
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
        let parent = match path.parent() {
            Some(p) => p,
            None => return Ok(None),
        };
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
        let value = match index {
            Some(index) => {
                let value = args[index + 1].clone();
                args.remove(index);
                args.remove(index);
                value
            }
            None => env::var("OTTOFILE").unwrap_or_else(|_| "./".to_owned()),
        };
        let ottofile = Self::divine_ottofile(value)?;
        Ok((ottofile, args))
    }

    fn indices(&self, args: &[String], task_names: &[&str]) -> Result<Vec<usize>> {
        let mut indices = vec![0];
        for (i, arg) in args.iter().enumerate() {
            if task_names.contains(&arg.as_str()) {
                indices.push(i);
            }
        }
        Ok(indices)
    }

    fn partitions(&self, args: &Vec<String>, task_names: &[&str]) -> Result<Vec<Vec<String>>> {
        let mut partitions = vec![];
        let mut end = args.len();
        for index in self.indices(args, task_names)?.iter().rev() {
            partitions.insert(0, args[*index..end].to_vec());
            end = *index;
        }
        Ok(partitions)
    }

    fn otto_to_command(otto: &Otto, with_subcommands: bool) -> Command {
        const OTTO_NAME: &str = "otto";
        const OTTO_ARG: &str = "ottofile";
        const OTTO_VAL: &str = "PATH";
        const OTTO_LONG: &str = "--ottofile";
        const OTTO_HELP: &str = "path to the ottofile";
        let mut command = Command::new(&otto.name)
            .bin_name(&otto.name)
            //.arg_required_else_help(true)
            .arg(
                Arg::new(OTTO_ARG)
                    .takes_value(true)
                    .value_name(OTTO_VAL)
                    .long(OTTO_LONG)
                    .help(OTTO_HELP),
            );
        if let Some(otto_help) = &otto.help {
            command = command.about(otto_help.as_str());
        }
        for param in otto.params.values() {
            command = command.arg(Self::param_to_arg(param));
        }
        if with_subcommands {
            for (name, task) in &otto.tasks {
                command = command.subcommand(Self::task_to_command(task));
            }
        }
        command
    }

    fn task_to_command(task: &Task) -> Command {
        let mut command = Command::new(task.name()).bin_name(task.name());
        if let Some(task_help) = task.help() {
            command = command.about(task.help());
        }
        for param in task.params().values() {
            command = command.arg(Self::param_to_arg(param));
        }
        command
    }
    fn param_to_arg(param: &dyn IParam) -> Arg {
        let mut arg = Arg::new(param.name());
        if let Some(short) = param.short() {
            arg = arg.short(short);
        }
        if let Some(long) = param.long() {
            arg = arg.long(long);
        }
        if *param.param_type() == ParamType::OPT {
            arg = arg.takes_value(true);
        }
        if let Some(help) = param.help() {
            arg = arg.help(help);
        }
        if let Some(default) = param.default() {
            arg = arg.default_value(default);
        }
        arg
    }

    pub fn parse(&self) -> Result<Vec<ArgMatches>> {
        let mut matches_vec = vec![];
        const OTTO_NAME: &str = "otto";
        const OTTO_ARG: &str = "ottofile";
        const OTTO_VAL: &str = "PATH";
        const OTTO_LONG: &str = "--ottofile";
        const OTTO_HELP: &str = "path to the ottofile";

        if let Some(ottofile) = &self.ottofile {
            //we have an ottofile, so let's load it
            let loader = Loader::new(ottofile);
            let spec = loader.load()?;
            let task_names = &spec.otto.task_names();
            let partitions = self.partitions(&self.args, task_names)?;
            //let mut otto = otto_command(&params, true);
            let otto = Command::new(OTTO_NAME)
                .bin_name(OTTO_NAME)
                .arg_required_else_help(true)
                .arg(
                    Arg::new(OTTO_ARG)
                        .takes_value(true)
                        .value_name(OTTO_VAL)
                        .long(OTTO_LONG)
                        .help(OTTO_HELP),
                );

            if !task_names.is_empty() {
                //we have tasks in the ottofile
                if partitions.len() == 1 {
                    //we only have the main otto partition; no tasks
                    let otto = Self::otto_to_command(&spec.otto, true)
                        .disable_help_subcommand(true)
                        .arg_required_else_help(true)
                        .after_help("after_help");
                    let matches = otto.get_matches_from(&partitions[0]);
                    matches_vec.push(matches);
                } else {
                    // we have multiple partions
                    // we need to add the task name to the command
                    // and then parse the args for each task
                    if let Some(index) = partitions
                        .iter()
                        .position(|p| !p.intersect(vec!["-h".to_owned(), "--help".to_owned()]).is_empty())
                    {
                        // we have a partition with help
                        // build the clap command for the partition with help
                        // and the parse the args
                        let partition = &partitions[index];
                        let task = &spec.otto.tasks[&partition[0]];
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
                        let otto = Self::otto_to_command(&spec.otto, false).after_help("otto!");
                        let otto_matches = otto.get_matches_from(&partitions[0]);
                        matches_vec.push(otto_matches);
                        for partition in &partitions[1..] {
                            let task = &spec.otto.tasks[&partition[0]];
                            let command = Self::task_to_command(task).after_help("task!");
                            let matches = command.get_matches_from(partition);
                            matches_vec.push(matches);
                        }
                    }
                }
            } else {
                // we don't have tasks in the ottofile
                panic!("no tasks in ottofile");
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
            let otto = Command::new(OTTO_NAME)
                .bin_name(OTTO_NAME)
                .arg_required_else_help(true)
                .after_help(after_help.as_str())
                .arg(
                    Arg::new(OTTO_ARG)
                        .takes_value(true)
                        .value_name(OTTO_VAL)
                        .long(OTTO_LONG)
                        .help(OTTO_HELP),
                );
            let matches = otto.get_matches_from(vec!["--help"]);
            matches_vec.push(matches);
        }
        Ok(matches_vec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_format_items() {
        let items = vec!["a", "b", "c"];
        let expected = "?a|b|c!";
        let formatted = format_items(&items, Some("?"), Some("|"), Some("!"));
        assert_eq!(expected, formatted);
    }
}
