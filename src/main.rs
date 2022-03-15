#![allow(unused_imports, unused_variables, dead_code)]

use otto::cli::parser::Parser;

fn main() {
    let parser = Parser::new();
    let matches = parser.parse();
    println!("matches={:#?}", matches);
}
