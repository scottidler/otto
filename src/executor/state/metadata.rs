use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Metadata about a run, stored in run.yaml and the database
/// This struct is shared between the file-based system and SQLite
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct RunMetadata {
    /// Path to the ottofile used for this run
    pub ottofile: Option<PathBuf>,

    /// Project hash (e.g., "6b20a2e4" from otto-6b20a2e4/)
    #[serde(default)]
    pub hash: String,

    /// Unix timestamp when run started (also used as directory name)
    #[serde(default)]
    pub timestamp: u64,

    /// Current working directory when run was executed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,

    /// Username who executed the run
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,

    /// Hostname where run was executed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,

    /// Command-line arguments (serialized as JSON string in DB)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,

    /// The directory this run writes into, recorded rather than reconstructed.
    /// Cleanup used to rebuild it from a naming convention it got wrong, so it
    /// deleted the database rows and left the directories on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_dir: Option<PathBuf>,
}

impl RunMetadata {
    /// Create minimal metadata (for backward compatibility with existing run.yaml files)
    pub fn minimal(ottofile: Option<PathBuf>, hash: String, timestamp: u64) -> Self {
        Self {
            ottofile,
            hash,
            timestamp,
            cwd: None,
            user: None,
            hostname: None,
            args: None,
            run_dir: None,
        }
    }

    pub fn full(
        ottofile: Option<PathBuf>,
        hash: String,
        timestamp: u64,
        cwd: Option<PathBuf>,
        user: Option<String>,
        hostname: Option<String>,
        args: Option<Vec<String>>,
    ) -> Self {
        Self {
            ottofile,
            hash,
            timestamp,
            cwd,
            user,
            hostname,
            args,
            run_dir: None,
        }
    }

    /// Record the directory this run writes into.
    pub fn with_run_dir(mut self, run_dir: PathBuf) -> Self {
        self.run_dir = Some(run_dir);
        self
    }

    /// Get current system metadata (user, hostname)
    pub fn current_system_info() -> (Option<String>, Option<String>) {
        let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).ok();

        let hostname = hostname::get().ok().and_then(|h| h.into_string().ok());

        (user, hostname)
    }
}

#[path = "metadata_tests.rs"]
mod tests;
