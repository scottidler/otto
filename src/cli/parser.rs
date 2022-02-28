#![allow(unused_imports, unused_variables, dead_code)]

use clap::{arg, Arg, ArgMatches, Command};
use std::env;
use std::path::PathBuf;

#[derive(Debug, Default, PartialEq)]
pub struct TaskArgs {
    name: String,
    args: Vec<String>,
    command: Option<Command<'static>>,
    matches: Option<ArgMatches>,
}

impl TaskArgs {
    pub fn new(name: String) -> Self {
        Self {
            name,
            args: vec![],
            command: None,
            matches: None,
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Parser {
    prog: String,
    args: Vec<String>,
}

impl Parser {
    pub fn new() -> Self {
        let mut args = env::args();
        Self {
            prog: args.next().unwrap(),
            args: args.collect(),
        }
    }
    pub fn parse(&self) -> Vec<TaskArgs> {
        vec![]
    }
    fn partition(&self) -> Vec<TaskArgs> {
        vec![]
    }
}
