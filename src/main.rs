#![allow(unused_imports, unused_variables, dead_code)]

use otto::cfg::loader;
use otto::cli::parser::Parser;

fn main() {
    let parser = Parser::new();
    println!("parser={:#?}", parser);
}
