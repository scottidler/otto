#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::Error;
use clap::{arg, Arg, ArgMatches, Command};

use std::collections::HashMap;
use std::env;
use std::fmt::Debug;
use std::ops::Range;
//use std::os::unix::raw::off_t;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::str::FromStr;
use std::unimplemented;

use crate::cfg::loader::Loader;
use crate::cfg::spec::{Nargs, Otto, Param, ParamType, Params, Spec, Task, Tasks, Value};

#[macro_use]
use super::macros;
use std::error;

const OTTOFILES: &'static [&'static str] = &["otto.yml", "otto.yaml", ".otto.yml", ".otto.yaml"];

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
    fn get_known_matches_from(&self, args: &Vec<String>) -> Result<(ArgMatches, Vec<String>), Error>;
}

impl<'a> GetKnownMatches for Command<'a> {
    fn get_known_matches(&self) -> Result<(ArgMatches, Vec<String>), Error> {
        let args: Vec<String> = env::args().collect();
        self.get_known_matches_from(&args)
    }
    fn get_known_matches_from(&self, args: &Vec<String>) -> Result<(ArgMatches, Vec<String>), Error> {
        let mut args_ = args.clone();
        let mut rem: Vec<String> = vec![];
        loop {
            match self.clone().try_get_matches_from(&*args_) {
                Ok(matches) => {
                    return Ok((matches, rem));
                }
                Err(error) => match error.kind() {
                    ErrorKind::UnknownArgument => {
                        let items = error.context().find_map(extract);
                        match items {
                            Some(ContextValue::String(s)) => {
                                rem.push(s.to_owned());
                                args_.retain(|a| a != s);
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

fn get_ottofile() -> String {
    env::var("OTTOFILE").unwrap_or_else(|_| OTTOFILE.to_owned())
}

#[derive(Debug, PartialEq)]
pub struct Parser<'a> {
    args: Vec<String>,
    ottofile: PathBuf,
    phantom: PhantomData<&'a str>,
}

impl<'a> Default for Parser<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Parser<'a> {
    pub fn new() -> Self {
        let args = env::args().collect();
        let ottofile = Parser::divine_ottofile();
        Self {
            args,
            ottofile,
            phantom: PhantomData,
        }
    }
    fn otto_command(nerfed: bool) -> Command<'a> {
        Command::new("otto")
            .bin_name("otto")
            .disable_help_flag(nerfed)
            .disable_version_flag(nerfed)
            .arg(
                Arg::new("ottofile")
                    .takes_value(true)
                    .short('o')
                    .long("ottofile")
                    .value_name("PATH")
                    .help("override default path to ottofile"),
            )
    }
    fn divine_ottofile() -> PathBuf {
        let otto_cmd = Parser::otto_command(true);
        let ottofile = match GetKnownMatches::get_known_matches(&otto_cmd) {
            Ok((matches, args)) => match matches.value_of("ottofile").map(str::to_string) {
                Some(s) => s,
                None => get_ottofile(),
            },
            Err(error) => get_ottofile(),
        };
        PathBuf::from(ottofile)
    }
    pub fn indices(&self, task_names: &[&str]) -> Result<Vec<usize>, Error> {
        let mut indices: Vec<usize> = vec![0];
        for (i, arg) in self.args.iter().enumerate() {
            if task_names.contains(&arg.as_str()) {
                indices.push(i);
            }
        }
        Ok(indices)
    }
    pub fn partitions(&self, task_names: &[&str]) -> Result<Vec<Vec<String>>, Error> {
        let mut partitions: Vec<Vec<String>> = vec![];
        let mut end = self.args.len();
        for index in self.indices(task_names)?.iter().rev() {
            partitions.insert(0, self.args[*index..end].to_vec());
            end = *index;
        }
        Ok(partitions)
    }
    pub fn parse(&self) -> Result<Vec<ArgMatches>, Error> {
        let mut matches_vec = vec![];
        if self.ottofile.exists() {
            let loader = Loader::new(&self.ottofile);
            let spec = loader.load().unwrap();
            let task_names = &spec.otto.task_names();
            if task_names.len() > 0 {
                let partitions = self.partitions(task_names)?;
                if partitions.len() == 1 {
                    let mut otto = Parser::otto_command(false).subcommand(Command::new("help").hide(true));
                    for task_name in task_names.iter() {
                        otto = otto.subcommand(Command::new(*task_name));
                    }
                    let args: Vec<String> = partitions[0][1..].to_vec();
                    let matches = otto.get_matches_from(&args[1..]);
                    matches_vec.push(matches);
                }
                for partition in partitions.iter().skip(1) {
                    let name = partition[0].clone();
                    let args: Vec<String> = partition[0..].to_vec();
                    let task = &spec.otto.tasks[&name];
                    let command = Parser::task_to_command(task);
                    let matches = command.get_matches_from(&args);
                    matches_vec.push(matches);
                }
            } else {
                let matches = Parser::otto_command(false).get_matches();
                matches_vec.push(matches);
            }
        } else {
            let after_help = format!("ottofile={:?} does not exist!", self.ottofile);
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
