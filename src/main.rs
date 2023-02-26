#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use std::str;
use eyre::Result;
use otto::cli::parser::Parser;

fn main() -> Result<()> {
    let parser = Parser::new()?;
    //println!("parser={:#?}", parser);
    match parser.parse() {
        Ok(matches_vec) => {
            for matches in matches_vec.iter() {
                println!("matches={:#?}", matches);
                println!("{}", "*".repeat(80));
            }
        }
        Err(error) => {
            println!("error={error:#?}");
        }
    }
    Ok(())
}
