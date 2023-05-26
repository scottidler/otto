//#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use std::env;
use eyre::Report;

use otto::cli::parse::Parser;
use otto::cmd::scheduler::Scheduler;

#[tokio::main]
async fn main() -> Result<(), Report> {
    let args: Vec<String> = env::args().collect();
    let mut parser = Parser::new(args)?;

    let (otto, jobs, hash) = parser.parse()?;
    let scheduler = Scheduler::new(otto, jobs, hash);
    scheduler.run_async().await?;

    Ok(())
}
