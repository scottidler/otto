#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use eyre::Result;
use otto::cli::parse::Parser;
use otto::cli::error::SilentError;
use std::str;
use std::process;

fn main() {
    let mut parser = match Parser::new() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error initializing parser: {e}");
            process::exit(1);
        }
    };

    match parser.parse() {
        Ok((otto, tasks)) => {
            println!("otto={otto:#?}");
            for task in tasks {
                println!("task={task:#?}");
            }
        }
        Err(e) => {
            if e.downcast_ref::<SilentError>().is_some() {
                process::exit(1);
            } else {
                eprintln!("Error parsing: {e}");
                process::exit(1);
            }
        }
    }
}
