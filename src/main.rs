// src/main.rs

use std::env;
use eyre::Report;

use otto::cli::parse::Parser;
use otto::cmd::scheduler::Scheduler;

#[tokio::main]
async fn main() -> Result<(), Report> {
    // Initialize logging with default level "warning"
    let env = env_logger::Env::default().filter_or("RUST_LOG", "warning");
    env_logger::Builder::from_env(env).init();

    let args: Vec<String> = env::args().collect();
    let mut parser = Parser::new(args)?;

    let (otto, jobs, hash) = parser.parse()?;
    let scheduler = Scheduler::new(otto, jobs, hash);
    scheduler.run_async().await?;

    Ok(())
}
