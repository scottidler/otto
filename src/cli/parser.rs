#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

//use clap::error::{ContextKind, ContextValue, ErrorKind};
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

use expanduser::expanduser;

use super::error::OttoParseError;
use crate::cfg::loader::Loader;
use crate::cfg::spec::{Nargs, Otto, Param, ParamType, Params, Spec, Task, Tasks, Value};

#[macro_use]
use super::macros;
use std::error;

const OTTOFILES: &'static [&'static str] = &[
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
    let mut path = expanduser(&value).unwrap(); //FIXME: should I handle the possible error?
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

#[derive(Debug, PartialEq)]
pub struct Parser<'a> {
    ottofile: Option<PathBuf>,
    args: Vec<String>,
    phantom: PhantomData<&'a str>,
}

impl<'a> Default for Parser<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Parser<'a> {
    pub fn new() -> Self {
        let (ottofile, args) = get_ottofile_args();
        Self {
            ottofile,
            args,
            phantom: PhantomData,
        }
    }
    fn otto_command(nerfed: bool) -> Command<'a> {
        Command::new("otto")
            .bin_name("otto")
            .disable_help_flag(nerfed)
            .disable_version_flag(nerfed)
            .arg(
                Arg::new("ottopath")
                    .takes_value(true)
                    .long("ottopath")
                    .value_name("PATH")
                    .help("override default ottopath"),
            )
    }

    pub fn indices(&self, args: &Vec<String>, task_names: &[&str]) -> Result<Vec<usize>, Error> {
        let mut indices: Vec<usize> = vec![0];
        for (i, arg) in args.iter().enumerate() {
            if task_names.contains(&arg.as_str()) {
                indices.push(i);
            }
        }
        Ok(indices)
    }
    pub fn partitions(&self, args: &Vec<String>, task_names: &[&str]) -> Result<Vec<Vec<String>>, Error> {
        let mut partitions: Vec<Vec<String>> = vec![];
        let mut end = args.len();
        for index in self.indices(args, task_names)?.iter().rev() {
            partitions.insert(0, args[*index..end].to_vec());
            end = *index;
        }
        Ok(partitions)
    }

    pub fn build_clap_for_otto_and_tasks(&self, spec: &Spec, args: &Vec<String>) -> ArgMatches {
        //tasks to vector of name, help tuples; convert help: Option<String> to String with default ""
        // this has to be done BEFORE the clap app is built
        let mut tasks: Vec<(String, String)> = spec
            .otto
            .tasks
            .iter()
            .map(|(name, task)| {
                (
                    name.clone(),
                    match &task.help {
                        Some(help) => help.to_owned(),
                        None => "".to_owned(),
                    },
                )
            })
            .collect();
        let mut otto = Parser::otto_command(true)
            .disable_help_subcommand(true)
            .arg_required_else_help(true)
            .after_help("after_help");
        for (name, help) in tasks.iter() {
            otto = otto.subcommand(
                Command::new(name.clone())
                    .about(help.as_str()) //this help.as_str() will cause lifetime issues
                    .arg(Arg::new("args").multiple_values(true)),
            );
        }
        otto.get_matches_from(args)
    }
    pub fn parse(&self) -> Result<Vec<ArgMatches>, OttoParseError> {
        let mut matches_vec = vec![];
        match &self.ottofile {
            Some(ottofile) => {
                let loader = Loader::new(&ottofile);
                let spec = loader.load()?;
                let task_names = &spec.otto.task_names();
                let partitions = self.partitions(&self.args, task_names)?;
                let mut otto = Parser::otto_command(true);

                if task_names.len() > 0 {
                    //we have tasks in the ottofile
                    if partitions.len() == 1 {
                        let matches = self.build_clap_for_otto_and_tasks(&spec, &partitions[0]);
                        matches_vec.push(matches);
                    } else {
                        // we have multiple partitions
                        // we need to add the task name to the command
                        // and then parse the args
                    }
                }
            }
            None => {
                // if we DON't have an ottofile
                // force the help message
                let mut otto = Parser::otto_command(false);
                // list ottofiles that can't be found
                let after_help = format!(
                    "ottofile not found in path or OTTOFILE env var\nOTTOFILES:\n- {0}",
                    OTTOFILES.join("\n- ")
                );
                let otto = Parser::otto_command(false)
                    .arg_required_else_help(true)
                    .after_help(after_help.as_str());
                let matches = otto.get_matches_from(vec!["--help"]);
            }
        }
        Ok(matches_vec)
    }

    /*
    fn otto_to_command(otto: &Otto) -> Command {
        unimplemented!()
    }
    */
    fn task_to_command(task: &Task) -> Command {
        let mut command = Command::new(&task.name);
        for param in task.params.values() {
            command = command.arg(Parser::param_to_arg(param));
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
}
