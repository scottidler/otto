#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use eyre::Result;
//use otto::cli::parse::Parser;
use otto::cli::parse2::Parser2;
use std::str;

fn main() -> Result<()> {
    //let mut parser = Parser::new()?;
    let mut parser = Parser2::new()?;
    let (defaults, tasks) = parser.parse()?;
    println!("defaults={:#?}", defaults);
    for task in tasks {
        println!("task={:#?}", task);
    }
    Ok(())
}
