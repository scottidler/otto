#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use eyre::Result;

use otto::cli::parse::Parser;
use otto::cli::error::SilentError;
use otto::cmd::scheduler::Scheduler;
use std::process;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut parser = match Parser::new(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error initializing parser: {e}");
            process::exit(1);
        }
    };

    let (otto, jobs) = parser.parse()?;

    println!("otto: {:?}", otto);
    println!("jobs: {:?}", jobs);

    println!("before scheduler");
    let sechedule = Scheduler::new(otto, jobs);
    sechedule.run();
    println!("after scheduler");

    Ok(())
}
