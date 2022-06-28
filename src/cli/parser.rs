#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::Error;
use clap::{arg, Arg, ArgMatches, Command};

use std::collections::HashMap;
use std::env;
use std::fmt::Debug;
use std::ops::Range;
use std::path::PathBuf;
use std::str::FromStr;

use crate::cfg::loader::Loader;
use crate::cfg::spec::{Nargs, Otto, Param, Params, Spec, Task, Tasks, Value};

#[macro_use]
use super::macros;
use super::error;

static OTTOFILE: &str = "./otto.yml";

fn print_type_of<T: ?Sized>(t: &T)
where
    T: Debug,
{
    println!("type={} value={:#?}", std::any::type_name::<T>(), t);
}

fn extract(item: (ContextKind, &ContextValue)) -> Option<&ContextValue> {
    let (k, v) = item;
    if k == ContextKind::InvalidArg {
        return Some(v);
    }
    None
}

pub trait GetKnownMatches {
    fn get_known_matches(&self) -> Result<(ArgMatches, Vec<String>), Error>;
    fn get_known_matches_from(&self, args: &mut Vec<String>) -> Result<(ArgMatches, Vec<String>), Error>;
}

impl<'a> GetKnownMatches for Command<'a> {
    fn get_known_matches(&self) -> Result<(ArgMatches, Vec<String>), Error> {
        let mut args: Vec<String> = env::args().collect();
        self.get_known_matches_from(&mut args)
    }
    fn get_known_matches_from(&self, args: &mut Vec<String>) -> Result<(ArgMatches, Vec<String>), Error> {
        let mut rem: Vec<String> = vec![];
        loop {
            match self.clone().try_get_matches_from(&*args) {
                Ok(matches) => {
                    return Ok((matches, rem));
                }
                Err(error) => match error.kind() {
                    ErrorKind::UnknownArgument => {
                        let items = error.context().find_map(extract);
                        match items {
                            Some(ContextValue::String(s)) => {
                                rem.push(s.to_owned());
                                args.retain(|a| a != s);
                            }
                            Some(&_) => {
                                return Err(error);
                            }
                            None => {
                                return Err(error);
                            }
                        }
                    }
                    _ => {
                        return Err(error);
                    }
                },
            }
        }
    }
}

/*
#[derive(Debug, Default, PartialEq, Clone)]
struct Partition {
    name: String,
    args: Vec<String>,
}
impl Partition {
    fn new(name: &str, args: &[String]) -> Self {
        Self {
            name: name.to_owned(),
            args: args.to_vec(),
        }
    }
    fn partition(tasknames: &[&str]) -> Vec<Partition> {
        let args: Vec<String> = env::args().collect();
        let mut partitions = vec![];
        let mut beg: usize = 0;
        for (i, arg) in args.iter().skip(1).enumerate() {
            if tasknames.iter().any(|t: &&str| t == arg) {
                let end = i + 1;
                partitions.push(Partition::new(&args[beg..end].join(" "), &args[beg..end]));
                beg = end;
            }
        }
        partitions.push(Partition::new("default", &args[beg..]));
        partitions
    }
}
*/

fn partition(task_names: &[&str]) -> HashMap<String, Vec<String>> {
    let args: Vec<String> = env::args().collect();
    let mut partitions = HashMap::new();
    let mut beg: usize = 0;
    for (i, arg) in args.iter().skip(1).enumerate() {
        if task_names.iter().any(|t: &&str| t == arg) {
            let end = i + 1;
            partitions.insert(arg.to_owned(), args[beg..end].to_vec());
            beg = end;
        }
    }
    partitions.insert("default".to_owned(), args[beg..].to_vec());
    partitions
}

#[derive(Debug, PartialEq)]
pub struct Parser {
    ottofile: String,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub fn new() -> Self {
        Self {
            ottofile: env::var("OTTOFILE").unwrap_or_else(|_| OTTOFILE.to_owned()),
        }
    }
    fn otto_seed(&self, nerfed: bool) -> Command {
        Command::new("otto")
            .disable_help_flag(nerfed)
            .disable_version_flag(nerfed)
            .arg(
                Arg::new("ottofile")
                    .takes_value(true)
                    .short('o')
                    .long("ottofile")
                    .help("override default path to ottofile"),
            )
    }
    pub fn divine_ottofile(&self) -> PathBuf {
        (match GetKnownMatches::get_known_matches(&self.otto_seed(true)) {
            Ok((matches, _)) => match matches.value_of("ottofile").map(str::to_string) {
                Some(s) => s,
                None => self.ottofile.clone(),
            },
            Err(error) => self.ottofile.clone(),
        }).into()
    }
    pub fn parse(&self) -> Vec<ArgMatches> {
        let ottofile = self.divine_ottofile();
        let mut matches_vec: Vec<ArgMatches> = vec![];
        if ottofile.exists() {
            let loader = Loader::new(&ottofile);
            let spec = loader.load().unwrap();
            let task_names = spec.otto.task_names();
            let partitions = partition(&task_names);
            for (name, args) in partitions {
                let task = &spec.otto.tasks[&name];
                let command = Parser::task_to_command(task);
                let mut args = args.clone();
                args.insert(0, name.to_owned());
                let matches = command.clone().try_get_matches_from(&args).unwrap();
                matches_vec.push(matches);
            }
        } else {
            let after_help = format!("ottofile={:?} does not exist!", ottofile);
            let otto = self
                .otto_seed(false)
                .arg_required_else_help(true)
                .after_help(after_help.as_str());
            let matches = otto.get_matches_from(vec!["--help"]);
            matches_vec.push(matches);
        }
        matches_vec
    }
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
        arg
    }
}
