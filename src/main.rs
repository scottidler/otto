#![allow(unused_imports, unused_variables, dead_code)]

use otto::cfg::loader::Loader;
use otto::cli::parser::Parser;

use clap::{Arg, Command};
use std::str::FromStr;

fn main() {
    let ottofile = Parser::divine_ottofile();
    println!("ottofile={:?}", ottofile);
    let loader = Loader::new(&ottofile);
    //let spec = loader.load().unwrap();
    let spec = match loader.load() {
        Ok(spec) => spec,
        Err(error) => {
            println!("error={:?}", error);
            panic!("couldn't find|load ottofile={:?}", ottofile)
        }
    };
    let parser = Parser::new(&spec);
    let matches = parser.parse();
    println!("matches={:#?}", matches);
}
