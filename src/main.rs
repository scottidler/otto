#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use eyre::Result;
use otto::cli::parse::Parser;
use std::str;

fn main() -> Result<()> {
    let parser = Parser::new()?;
    //println!("parser={:#?}", parser);
    let spec = parser.parse();
    Ok(())
}
