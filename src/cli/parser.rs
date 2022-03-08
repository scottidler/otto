#![allow(unused_imports, unused_variables, unused_attributes, dead_code)]

use crate::cfg::spec::{Nargs, Otto, Param, Spec, Task, Value};
use clap::{arg, Arg, ArgMatches, Command};
use std::env;
use std::ops::Range;
use std::path::PathBuf;

#[macro_use]
use super::macros;
use super::error;

fn print_type_of<T: ?Sized>(_: &T) {
    println!("{}", std::any::type_name::<T>())
}

#[derive(Debug, Default, PartialEq)]
pub struct Parser {
    pub spec: Spec,
    pub prog: String,
    pub args: Vec<String>,
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

impl Parser {
    pub fn new(spec: Spec) -> Self {
        let mut args = env::args();
        Self {
            spec,
            prog: args.next().unwrap(),
            args: args.collect(),
        }
    }
    /*
    pub fn from_yaml<'a>(param: Param) -> Arg<'a> {
        let mut arg = Arg::new(param.name.as_str());
        for (k, v) in param.to_kv() {
            arg = match k.as_str().expect("Arg fields must be strings") {
                "short" => str_to_char!(arg, v, short),
                "long" => str_to_str!(arg, v, long),
                //"aliases" => str_vec_or_str!(arg, v, alias),
                "help" => str_to_str!(arg, v, help),
                //"long_help" => str_to_str!(arg, v, long_help),
                "required" => str_to_bool!(arg, v, required),
                //"required_if" => str_tuple2!(arg, v, required_if_eq),
                //"required_ifs" => str_tuple2!(arg, v, required_if_eq),
                "takes_value" => str_to_bool!(arg, v, takes_value),
                //"index" => str_to_usize!(arg, v, index),
                //"global" => str_to_bool!(arg, v, global),
                //"multiple" => str_to_bool!(arg, v, multiple_values), //note: changed multiple -> multiple_values (compiler warning)
                //"hidden" => str_to_bool!(arg, v, hide),
                //"next_line_help" => str_to_bool!(arg, v, next_line_help),
                //"group" => str_to_str!(arg, v, group),
                //"number_of_values" => str_to_usize!(arg, v, number_of_values),
                //"max_values" => str_to_usize!(arg, v, max_values),
                //"min_values" => str_to_usize!(arg, v, min_values),
                "value_name" => str_to_str!(arg, v, value_name),
                //"use_delimiter" => str_to_bool!(arg, v, use_delimiter),
                "allow_hyphen_values" => str_to_bool!(arg, v, allow_hyphen_values),
                "last" => str_to_bool!(arg, v, last),
                //"require_delimiter" => str_to_bool!(arg, v, require_delimiter),
                //"value_delimiter" => str_to_char!(arg, v, value_delimiter),
                //"required_unless" => str_to_str!(arg, v, required_unless_present),
                //"display_order" => str_to_usize!(arg, v, display_order),
                "default_value" => str_to_str!(arg, v, default_value),
                //"default_value_if" => str_tuple3!(arg, v, default_value_if),
                //"default_value_ifs" => str_tuple3!(arg, v, default_value_if),
                /*
                #[cfg(feature = "env")]
                "env" => str_to_str!(arg, v, env),
                "value_names" => str_vec_or_str!(arg, v, value_name),
                "groups" => str_vec_or_str!(arg, v, group),
                "requires" => str_vec_or_str!(arg, v, requires),
                "requires_if" => str_tuple2!(arg, v, requires_if),
                "requires_ifs" => str_tuple2!(arg, v, requires_if),
                "conflicts_with" => str_vec_or_str!(arg, v, conflicts_with),
                "overrides_with" => str_to_str!(arg, v, overrides_with),
                "possible_values" => str_vec_or_str!(arg, v, possible_value),
                "case_insensitive" => str_to_bool!(arg, v, ignore_case),
                "required_unless_one" => str_vec!(arg, v, required_unless_present_any),
                "required_unless_all" => str_vec!(arg, v, required_unless_present_all),
                */
                s => {
                    panic!(
                        "Unknown setting '{}' in YAML file for arg '{}'",
                        s, name_str
                    )
                }
            }
        }
        arg
    }
    */
    fn task_names(&self) -> Vec<&str> {
        self.spec
            .otto
            .tasks
            .keys()
            .map(AsRef::as_ref)
            .collect::<Vec<&str>>()
    }
    fn tasks(&self) -> Vec<&Task> {
        self.spec.otto.tasks.values().collect()
    }

    pub fn parse(&self) -> Vec<ArgMatches> {
        let task_names = self.task_names();
        println!("task_names={:#?}", task_names);
        let pa = PartitionedArgs::new(&task_names);
        println!("pa={:#?}", pa);
        vec![]
    }
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
}
