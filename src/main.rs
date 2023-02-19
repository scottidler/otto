#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use eyre::Result;
use otto::cli::parser::Parser;
use otto::cli::parser2::Parser2;

fn main() -> Result<()> {
    //let parser = Parser::new();
    let parser = Parser2::new()?;
    //println!("parser={:#?}", parser);
    match parser.parse() {
        Ok(matches) => {
            println!("matches={matches:#?}");
        }
        Err(error) => {
            println!("error={error:#?}");
        }
    }
    Ok(())
}
