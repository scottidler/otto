#![allow(unused_imports, unused_variables, dead_code)]

use otto::cfg::loader::Loader;
use otto::cli::parser::Parser;

fn main() {
    let ottofile = "examples/ex1.yml";
    let loader = Loader::new();
    let spec = loader.load(ottofile).unwrap();
    let parser = Parser::new(spec);
    let matches = parser.parse();
    println!("matches={:#?}", matches);
}
