#![allow(unused_imports, unused_variables, dead_code)]

use otto::cfg::loader::Loader;
use otto::cli::parser::Parser;

fn main() {
    let parser = Parser::new();
    let loader = Loader::new();
    println!("parser.ottofile={:#?}", parser.ottofile);
    let spec = loader.load(&parser.ottofile).unwrap();
    let matches = parser.parse(&spec);
    println!("matches={:#?}", matches);
}
