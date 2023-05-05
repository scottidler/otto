#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use eyre::Result;
use otto::cli::parse::Parser;
use std::str;

fn main() -> Result<()> {
    let mut parser = Parser::new()?;
    let (defaults, tasks) = parser.parse()?;
    println!("defaults={:#?}", defaults);
    for task in tasks {
        println!("task={:#?}", task);
    }
    Ok(())
}
