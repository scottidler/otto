#![allow(unused_imports, unused_variables, dead_code)]

use otto::cli::parser::Parser;

fn main() {
    let parser = Parser::new();
    //println!("parser={:#?}", parser);
    match parser.parse() {
        Ok(matches) => {
            println!("matches={:#?}", matches);
        }
        Err(error) => {
            println!("error={:#?}", error);
        }
    }
}
