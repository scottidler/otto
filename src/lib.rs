pub mod app;
pub mod cfg;
pub mod cli;
pub mod executor;
pub mod makefile;
pub mod ports;
pub mod tui;
pub mod utils;

pub use app::{RuntimeConfig, Startup, run};
pub use cfg::config::ConfigSpec;
pub use cli::Parser;
pub use executor::{Task, TaskScheduler, Workspace};
pub use ports::{FileSystem, MemFs, RealFs};

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}
