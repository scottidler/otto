#![allow(unused_imports, unused_variables, unused_attributes, dead_code)]

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
use crate::cfg::spec::{Nargs, Otto, Param, Spec, Task, Value};

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
#[derive(Debug, Default, PartialEq, Clone)]
struct PartitionedArgs {
    args: Vec<String>,
    partitions: Vec<Range<usize>>,
}
impl PartitionedArgs {
    fn new(tasknames: &[&str]) -> Self {
        let args: Vec<String> = env::args().collect();
        let mut beg = 0;
        let mut partitions = vec![];
        for (i, arg) in args.iter().skip(1).enumerate() {
            if tasknames.iter().any(|t| t == arg) {
                let end = i + 1;
                partitions.push(Range::<usize> { start: beg, end });
                beg = end;
            }
        }
        partitions.push(Range::<usize> {
            start: beg,
            end: args.len(),
        });
        Self { args, partitions }
    }
    fn partitions(&self) -> Vec<&[String]> {
        self.partitions.iter().map(|p| &self.args[p.clone()]).collect()
    }
    fn partition(&self, index: usize) -> Option<&[String]> {
        if index < self.len() {
            return Some(&self.args[self.partitions[index].clone()]);
        }
        None
    }
    fn len(&self) -> usize {
        self.partitions.len()
    }
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
        let ottofile = match self.otto_seed(true).get_known_matches() {
            Ok((matches, _)) => match matches.value_of("ottofile").map(str::to_string) {
                Some(s) => s,
                None => self.ottofile.clone(),
            },
            Err(error) => self.ottofile.clone(),
        };
        ottofile.into()
    }
    pub fn parse(&self) -> Vec<ArgMatches> {
        let ottofile = self.divine_ottofile();
        if ottofile.exists() {
            let loader = Loader::new(&ottofile);
            let spec = loader.load().unwrap();
            let task_names = spec.otto.task_names();
            let mut commands = HashMap::<&str, Command>::new();
            for task_name in task_names.clone() {
                let task = &spec.otto.tasks[task_name];
                let command = Parser::task_to_command(task);
                commands.insert(task_name, command);
            }
            let pa = PartitionedArgs::new(&task_names.clone());
        } else {
            let after_help = format!("ottofile={:?} does not exist!", ottofile);
            let otto = self
                .otto_seed(false)
                .arg_required_else_help(true)
                .after_help(after_help.as_str());
            let matches = otto.get_matches_from(vec!["--help"]);
        }
        vec![]
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
        if let Some(short) = &param.short {
            arg = arg.short(short.chars().next().unwrap());
        }
        if let Some(long) = &param.long {
            arg = arg.long(long);
        }
        arg
    }
}
