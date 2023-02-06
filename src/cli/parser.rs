#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

//use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::Error;
use clap::{arg, Arg, ArgMatches, Command};
use thiserror::Error;

use std::collections::HashMap;
use std::env;
use std::fmt::Debug;
use std::fs;
use std::fs::metadata;
use std::marker::PhantomData;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::unimplemented;

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

#[derive(Error, Debug)]
pub enum OttofileError {
    #[error("env var error: {0}")]
    HomeUndefined(#[from] env::VarError),
    #[error("canonicalize error")]
    CanoncalizeError(#[from] std::io::Error),
    #[error("divinie error; unable to find ottofile from path=[{0}]")]
    DivineError(PathBuf),
    #[error("relative path error")]
    RelativePathError,
    #[error("unknown error")]
    Unknown,
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

fn find_ottofile_old(path: &Path) -> Option<PathBuf> {
    for ottofile in OTTOFILES {
        let ottofile_path = path.join(ottofile);
        if ottofile_path.exists() {
            return Some(ottofile_path);
        }
    }
    None
}

fn find_ottofile(path: &Path) -> Option<PathBuf> {
    for ottofile in OTTOFILES {
        let ottofile_path = path.join(ottofile);
        if ottofile_path.exists() {
            return Some(ottofile_path);
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

fn divine_ottofile(value: String) -> Result<PathBuf, OttofileError> {
    let cwd = env::current_dir()?;
    let home = env::var("HOME")?;
    let mut path = PathBuf::from(value.replace("~", home.as_str()));
    path = fs::canonicalize(path)?;
    if path.is_dir() {
        if let Some(ottofile) = find_ottofile(&path) {
            path = ottofile;
        } else {
            return Err(OttofileError::DivineError(path));
        }
    }
    path = path_relative_from(&path, &cwd).ok_or(OttofileError::RelativePathError)?;
    Ok(path)
}

fn get_ottofile_args() -> Result<(PathBuf, Vec<String>), OttofileError> {
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
    let ottofile = divine_ottofile(value)?;
    Ok((ottofile, args))
}
#[derive(Debug, PartialEq)]
pub struct Parser<'a> {
    phantom: PhantomData<&'a str>,
}

impl<'a> Default for Parser<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Parser<'a> {
    pub fn new() -> Self {
        Self {
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
                    .short('o')
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
    pub fn parse(&self) -> Result<Vec<ArgMatches>, Error> {
        let mut matches_vec = vec![];
        let (ottofile, mut args) = get_ottofile_args().unwrap();
        if ottofile.exists() {
            // if we have an ottofile
            println!("ottofile={:?}", ottofile);
            let loader = Loader::new(&ottofile);
            let spec = loader.load().unwrap();
            let task_names = &spec.otto.task_names();
            let partitions = self.partitions(&args, task_names)?;
            println!("partitions={:?}", partitions);
            let partition = &partitions[0];
            println!("first partition={:?}", partition);
            let mut otto = Parser::otto_command(false);
            let param_names = &spec.otto.param_names();
            if param_names.len() > 0 {
                println!("param_names={:?}", param_names);
                for param in spec.otto.params.values() {
                    otto = otto.arg(Parser::param_to_arg(param));
                }
            }
            let name = partition[0].clone();
            let args: Vec<String> = partition[0..].to_vec();
            let matches = otto.clone().get_matches_from(&args[1..]);
            matches_vec.push(matches);
            println!("name={:?} args={:?}", name, args);
            if task_names.len() > 0 {
                // if we have tasks in ottofile
                //let partitions = self.partitions(task_names)?;
                if partitions.len() == 1 {
                    // if we have't matched any task partitions
                    //let mut otto = Parser::otto_command(false).subcommand(Command::new("help").hide(true));
                    otto = otto.subcommand(Command::new("help").hide(true));
                    for task_name in task_names.iter() {
                        otto = otto.subcommand(Command::new(*task_name));
                    }
                    //let args: Vec<String> = partitions[0][1..].to_vec();
                    let args: Vec<String> = partitions[0].to_vec();
                    let matches = otto.get_matches_from(&args[1..]);
                    matches_vec.push(matches);
                } else {
                    // we have matched some task partitions
                    for partition in partitions.iter().skip(1) {
                        let name = partition[0].clone();
                        let args: Vec<String> = partition[0..].to_vec();
                        let task = &spec.otto.tasks[&name];
                        let command = Parser::task_to_command(task);
                        let matches = command.get_matches_from(&args);
                        matches_vec.push(matches);
                    }
                }
            }
            /*else {
                // FIXME: should allow the parser to not have tasks
                let matches = Parser::otto_command(false).get_matches();
                matches_vec.push(matches);
            }*/
        } else {
            // if we DON't have an ottofile
            let after_help = format!("ottofile={:?} does not exist!", ottofile);
            let otto = Parser::otto_command(false)
                .arg_required_else_help(true)
                .after_help(after_help.as_str());
            let matches = otto.get_matches_from(vec!["--help"]);
            matches_vec.push(matches);
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
