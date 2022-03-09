#![allow(unused_imports, unused_variables, unused_attributes, dead_code)]

use crate::cfg::spec::{Nargs, Otto, Param, Spec, Task, Value};
use clap::{arg, Arg, ArgMatches, Command};
use std::env;
use std::fmt::Debug;
use std::ops::Range;
use std::path::PathBuf;
use std::str::FromStr;

#[macro_use]
use super::macros;
use super::error;

fn print_type_of<T: ?Sized>(t: &T)
where
    T: Debug,
{
    println!("type={} value={:#?}", std::any::type_name::<T>(), t);
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

#[derive(Debug, Default, PartialEq)]
pub struct Parser {
    pub jobs: usize,
    pub prog: String,
    pub args: Vec<String>,
    //pub ottofile: String,
    pub ottofile: PathBuf,
}
/*
fn otto_parser() -> Command {
    Command::new("otto").arg(
        Arg::new("ottofile")
            .takes_value(true)
            .short('o')
            .long("ottofile")
            .help("path to ottofile"),
    )
}
*/

impl Parser {
    pub fn new() -> Self {
        let args: Vec<String> = env::args().collect();
        let prog = args[0].clone();
        let jobs = 4; //dynamically default to nproc
        let ottofile = "./otto.yml".to_owned();
        let matches = Command::new("otto")
            .arg(
                Arg::new("ottofile")
                    .takes_value(true)
                    .short('o')
                    .long("ottofile")
                    .help("path to ottofile"),
            )
            .get_matches_from(&args);
        let ottofile =
            PathBuf::from_str(matches.value_of("ottofile").unwrap_or("./otto.yml")).unwrap();
        Self {
            jobs,
            prog,
            args,
            ottofile,
        }
    }

    pub fn parse(&self, spec: &Spec) -> Vec<ArgMatches> {
        let task_names = spec
            .otto
            .tasks
            .keys()
            .map(AsRef::as_ref)
            .collect::<Vec<&str>>();
        println!("task_names={:#?}", task_names);
        let pa = PartitionedArgs::new(&task_names);
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
