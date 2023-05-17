//#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use otto::cli::parse::Parser;
use otto::cli::error::SilentError;
use std::process;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut parser = match Parser::new(args) {
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
