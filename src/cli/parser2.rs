#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

//use clap::error::{ContextKind, ContextValue, ErrorKind};
use eyre::{eyre, Result};
use clap::Error;
use clap::{arg, Arg, ArgMatches, Command};
use thiserror::Error;

use std::collections::HashMap;
use std::env;
use std::fmt::{Debug, Display, Formatter};
use std::fs;
use std::fs::metadata;
use std::marker::PhantomData;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::unimplemented;
use std::ffi::OsStr;

use expanduser::expanduser;
use array_tool::vec::Intersect;

use super::error::OttoParseError;
use crate::cfg::loader::Loader;
use crate::cfg::spec::{Nargs, Otto, Param, ParamType, Params, Spec, Task, Tasks, Value};

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
/*

const OTTO_NAME: &str = "otto";
const OTTO_ARG: &str = "ottofile";
const OTTO_VAL: &str = "PATH";
const OTTO_LONG: &str = "--ottofile";
const OTTO_HELP: &str = "path to the ottofile";

*/

fn print_type_of<T: ?Sized>(t: &T)
where
    T: Debug,
{
    println!("type={} value={:#?}", std::any::type_name::<T>(), t);
}

fn decor(items: &[&str], pre: Option<&str>, mid: Option<&str>, post: Option<&str>) -> String
where
{
    //if mid is not None, then join with mid
    //if mid is None, then join with ""
    let mut s = match mid {
        Some(mid) => items.join(mid),
        None => items.join(""),
    };
    //if pre is not None, then prepend with pre
    if let Some(pre) = pre {
        s = format!("{pre}{s}");
    }
    //if post is not None, then append with post
    if let Some(post) = post {
        s = format!("{s}{post}");
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

fn find_ottofile(path: &Path) -> Option<PathBuf> {
    let cwd = env::current_dir().unwrap(); //FIXME: should I handle the possible error?
    for ottofile in OTTOFILES {
        let ottofile_path = path.join(ottofile);
        if ottofile_path.exists() {
            match path_relative_from(path, &cwd) {
                Some(p) => return Some(p),
                None => return None,
            }
        }
    }
    let parent = match path.parent() {
        Some(p) => p,
        None => return None,
    };
    if parent == Path::new("/") {
        return None;
    }
    find_ottofile(parent)
}

fn divine_ottofile(value: String) -> Option<PathBuf> {
    let mut path = expanduser(value).unwrap(); //FIXME: should I handle the possible error?
    path = fs::canonicalize(path).unwrap(); //FIXME: should I handle the possible error?
    if path.is_dir() {
        match find_ottofile(&path) {
            Some(path) => return Some(path),
            None => return None,
        }
    }
    Some(path)
}

fn get_ottofile_args() -> (Option<PathBuf>, Vec<String>) {
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
    let ottofile = divine_ottofile(value);
    (ottofile, args)
}

fn prog() -> Result<String> {
    Ok(std::env::current_exe()?
        .file_name()
        .ok_or_else(|| eyre::eyre!("could not get file name"))?
        .to_str()
        .ok_or_else(|| eyre::eyre!("could not convert to str"))?
        .to_owned())
}

/*
impl Default for Parser2 {
    fn default() -> Self {
        let (ottofile, args) = get_ottofile_args();
        let cwd = env::current_dir().unwrap(); //FIXME: should I handle the possible error?
        Self {
            ottofile,
            cwd,
            args,
        }
    }
}
*/

/*
#[derive(Debug, PartialEq)]
struct OttoCommandParams {
    name: String,
    arg: String,
    val: String,
    long: String,
    help: String,
}

impl OttoCommandParams {
    fn new(nerfed: bool, name: String, arg: String, val: String, long: String, help: String) -> Self {
        Self {
            name,
            arg,
            val,
            long,
            help,
        }
    }
}

fn otto_command(params: &OttoCommandParams, nerfed: bool) -> Command {
    Command::new(params.name.as_str())
        .bin_name(params.name.as_str())
        .disable_help_flag(nerfed)
        .disable_version_flag(nerfed)
        .arg(
            Arg::new(params.arg.as_str())
                .takes_value(true)
                .value_name(params.val.as_str())
                .long(params.long.as_str())
                .help(params.help.as_str())
        )
}
*/
/*
fn build_clap_command_from_task(task: &Task, partition: &Vec<String>) -> Command {
    let mut command = Command::new(task.name.as_str())
        .bin_name(task.name.as_str())
        .about(task.help.as_str());
        for param in &task.params {
        }
    command
}
*/

fn otto_to_command(otto: &Otto, with_subcommands: bool) -> Command {
    let mut command = Command::new(&otto.name)
        .bin_name(&otto.name);
    if let Some(otto_help) = &otto.help {
        command = command.about(otto_help.as_str());
    }
    for param in otto.params.values() {
        command = command.arg(param_to_arg(param));
    }
    if with_subcommands {
        for (name, task) in &otto.tasks {
            command = command.subcommand(task_to_command(task));
        }
    }
    command
}

fn task_to_command(task: &Task) -> Command {
    let mut command = Command::new(&task.name)
        .bin_name(&task.name);
    if let Some(task_help) = &task.help {
        command = command.about(task_help.as_str());
    }
    for param in task.params.values() {
        command = command.arg(param_to_arg(param));
    }
    command
}
fn param_to_arg(param: &Param) -> Arg {
    let mut arg = Arg::new(&*param.name);
    if let Some(short) = param.short {
        arg = arg.short(short);
    }
    if let Some(long) = &param.long {
        arg = arg.long(long.as_str());
    }
    if param.param_type == ParamType::OPT {
        arg = arg.takes_value(true);
    }
    if let Some(help) = &param.help {
        arg = arg.help(help.as_str());
    }
    if let Some(default) = &param.default {
        arg = arg.default_value(default.as_str());
    }
    arg
}

#[derive(Debug, PartialEq)]
pub struct Parser2 {
    prog: String,
    cwd: PathBuf,
    user: String,
    ottofile: Option<PathBuf>,
    args: Vec<String>,
}

impl Parser2 {

    pub fn new() -> Result<Self> {
        let prog = prog()?;
        let cwd = env::current_dir()?;
        let user = env::var("USER")?;
        let (ottofile, args) = get_ottofile_args();
        Ok(Self {
            prog,
            cwd,
            user,
            ottofile,
            args,
        })
    }

    pub fn indices(&self, args: &Vec<String>, task_names: &[&str]) -> Result<Vec<usize>> {
        let mut indices = vec![0];
        for (i, arg) in args.iter().enumerate() {
            if task_names.contains(&arg.as_str()) {
                indices.push(i);
            }
        }
        Ok(indices)
    }

    pub fn partitions(&self, args: &Vec<String>, task_names: &[&str]) -> Result<Vec<Vec<String>>> {
        let mut partitions = vec![];
        let mut end = args.len();
        for index in self.indices(args, task_names)?.iter().rev() {
            partitions.insert(0, args[*index..end].to_vec());
            end = *index;
        }
        Ok(partitions)
    }

    pub fn parse(&self) -> Result<Vec<ArgMatches>> {
        let mut matches_vec = vec![];
        const OTTO_NAME: &str = "otto";
        const OTTO_ARG: &str = "ottofile";
        const OTTO_VAL: &str = "PATH";
        const OTTO_LONG: &str = "--ottofile";
        const OTTO_HELP: &str = "path to the ottofile";
        /*
        fn otto_command(params: &OttoCommandParams, nerfed: bool) -> Command {
            Command::new(params.name.as_str())
                .bin_name(params.name.as_str())
                .disable_help_flag(nerfed)
                .disable_version_flag(nerfed)
                .arg(
                    Arg::new(params.arg.as_str())
                        .takes_value(true)
                        .value_name(params.val.as_str())
                        .long(params.long.as_str())
                        .help(params.help.as_str())
                )
        }
        */
        /*
        let params = OttoCommandParams::new(
            true,
            "otto".to_owned(),
            "ottofile".to_owned(),
            "PATH".to_owned(),
            "--ottofile".to_owned(),
            "path to the ottofile".to_owned(),
        );
        */
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
                    .arg(Arg::new(OTTO_ARG)
                    .takes_value(true)
                    .value_name(OTTO_VAL)
                    .long(OTTO_LONG)
                    .help(OTTO_HELP)
                );

            if !task_names.is_empty() {
                //we have tasks in the ottofile
                if partitions.len() == 1 {
                    //we only have the main otto partition; no tasks
                    let otto = otto_to_command(&spec.otto, false)
                        .disable_help_subcommand(true)
                        .arg_required_else_help(true)
                        .after_help("after_help");
                    let matches = otto.get_matches_from(&partitions[0]);
                    matches_vec.push(matches);
                } else {
                    // we have multiple partions
                    // we need to add the task name to the command
                    // and then parse the args for each task

                    fn contains_help(partition: &Vec<String>) -> bool {
                        !partition
                            .intersect(vec!["-h".to_owned(), "--help".to_owned()])
                            .is_empty()
                    }

                    if let Some(index) = partitions.iter().position(|p| contains_help(p)) {
                        // we have a partition with help
                        // build the clap command for the partition with help
                        // and the parse the args
                        let partition = &partitions[index];
                        let task = &spec.otto.tasks[&partition[0]];
                        let command = task_to_command(task)
                            .disable_help_subcommand(true)
                            .arg_required_else_help(true)
                            .after_help("after_help");
                        let matches = command.get_matches_from(partition);
                        matches_vec.push(matches);
                    } else {
                        // we don't have a partition with help
                        // build the clap command for the otto and tasks, respectively
                        // and then parse the args (partition) for each task

                    }

                }

            } else {
                // we don't have tasks in the ottofile
                panic!("no tasks in ottofile");
            }
        } else {
            // if we don't have an ottofile
            // force the help message
            /*
            let params = OttoCommandParams::new(
                true,
                "otto".to_owned(),
                "ottofile".to_owned(),
                "PATH".to_owned(),
                "--ottofile".to_owned(),
                "path to the ottofile".to_owned(),
            );
            */
            let dash = "\n- ";
            let after_help = format!(
                "--ottofile arg not specified, nor OTTOFILE env var, not one of 'OTTOFILES' discovered in path={0}\nOTTOFILES: {1}",
                self.cwd.display(),
                decor(OTTOFILES, Some(dash), Some(dash), None)
            );
            let otto = Command::new(OTTO_NAME)
                .bin_name(OTTO_NAME)
                .arg_required_else_help(true)
                .after_help(after_help.as_str())
                .arg(Arg::new(OTTO_ARG)
                    .takes_value(true)
                    .value_name(OTTO_VAL)
                    .long(OTTO_LONG)
                    .help(OTTO_HELP)
                );
            let matches = otto.get_matches_from(vec!["--help"]);
            matches_vec.push(matches);
        }
        Ok(matches_vec)
    }
}