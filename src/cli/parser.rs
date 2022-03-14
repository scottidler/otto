#![allow(unused_imports, unused_variables, unused_attributes, dead_code)]

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::Error;
use clap::{arg, Arg, ArgMatches, Command};

use std::env;
use std::fmt::Debug;
use std::ops::Range;
use std::path::PathBuf;
use std::str::FromStr;

use crate::cfg::spec::{Nargs, Otto, Param, Spec, Task, Value};

#[macro_use]
use super::macros;
use super::error;

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
    fn get_known_matches_from(
        &self,
        args: &mut Vec<String>,
    ) -> Result<(ArgMatches, Vec<String>), Error>;
}

impl<'a> GetKnownMatches for Command<'a> {
    fn get_known_matches(&self) -> Result<(ArgMatches, Vec<String>), Error> {
        let mut args: Vec<String> = env::args().collect();
        self.get_known_matches_from(&mut args)
    }
    fn get_known_matches_from(
        &self,
        args: &mut Vec<String>,
    ) -> Result<(ArgMatches, Vec<String>), Error> {
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
                            Some(x) => match x {
                                ContextValue::String(s) => {
                                    rem.push(s.to_owned());
                                    args.retain(|a| a != s);
                                }
                                _ => {
                                    return Err(error);
                                }
                            },
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
        self.partitions
            .iter()
            .map(|p| &self.args[p.clone()])
            .collect()
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

fn otto_seed() -> Command<'static> {
    Command::new("otto")
        .disable_help_flag(true)
        .disable_version_flag(true)
        .arg(
            Arg::new("ottofile")
                .takes_value(true)
                .short('o')
                .long("ottofile")
                .help("path to ottofile"),
        )
}

#[derive(Debug, PartialEq)]
pub struct Parser<'a> {
    spec: &'a Spec,
}

impl<'a> Parser<'a> {
    pub fn new(spec: &'a Spec) -> Self {
        Self { spec }
    }
    fn task_names(&self) -> Vec<&str> {
        self.spec.otto.tasks.keys().map(AsRef::as_ref).collect()
    }
    pub fn divine_ottofile() -> PathBuf {
        let ottofile = match otto_seed().get_known_matches() {
            Ok((matches, _)) => {
                let result = matches.value_of("ottofile");
                println!("result={:?}", result);
                result.unwrap_or("./otto.yml").to_string()
            }
            Err(error) => {
                println!("divine: error={}", error);
                "./otto.yml".to_string()
            }
        };
        PathBuf::from_str(&ottofile).unwrap()
    }

    pub fn parse(&self) -> Vec<ArgMatches> {
        println!("task_names={:#?}", self.task_names());
        let pa = PartitionedArgs::new(&self.task_names());
        println!("pa={:#?}", pa);
        /*
        print_type_of(&args);
        println!("otto_args={:#?}", otto_args);
        print_type_of(&otto_args);
        let matches = self.otto_parser().get_matches_from(otto_args);
        print_type_of(&matches);
        let ottofile = matches.value_of("ottofile").unwrap_or("./");
        print_type_of(&ottofile);
        let mut pathbuf = PathBuf::new();
        pathbuf.push(ottofile);
        print_type_of(&pathbuf);
        let exists = pathbuf.exists();
        print_type_of(&exists);
        */
        vec![]
    }
    /*
    fn tasks_to_commands(&self) -> Vec<Command> {
        fn param_to_arg(param: &Param) -> Arg {
            println!("param={:#?}", param);
            let mut arg = Arg::new(&*param.name);
            if let Some(help) = &param.help {
                println!("help={:?}", help);
                arg = arg.help(Some(&help[..]));
            }
            arg
        }
        fn task_to_command(task: &Task) -> Command {
            let args: Vec<Arg> = task.params.values().map(param_to_arg).collect();
            Command::new(task.name.to_owned()).args(args)
        }
        self.tasks().iter().map(|t| task_to_command(t)).collect()
    }
    */
}
