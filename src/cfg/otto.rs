// cfg/otto.rs

use serde::Deserialize;
use std::vec::Vec;

fn default_name() -> String {
    "otto".to_string()
}

fn default_about() -> String {
    "A task runner".to_string()
}

fn default_api() -> String {
    "1".to_string()
}

fn default_jobs() -> usize {
    num_cpus::get()
}

fn default_home() -> String {
    "~/.otto".to_string()
}

fn default_tasks() -> Vec<String> {
    vec!["*".to_string()]
}

fn default_verbosity() -> String {
    "1".to_string()
}

#[must_use]
pub fn default_otto() -> Otto {
    Otto {
        name: default_name(),
        about: default_about(),
        api: default_api(),
        jobs: default_jobs(),
        home: default_home(),
        tasks: default_tasks(),
        verbosity: default_verbosity(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Otto {
    #[serde(default = "default_name")]
    pub name: String,

    #[serde(default = "default_about")]
    pub about: String,

    #[serde(default = "default_api")]
    pub api: String,

    #[serde(default = "default_jobs")]
    pub jobs: usize,

    #[serde(default = "default_home")]
    pub home: String,

    #[serde(default = "default_tasks")]
    pub tasks: Vec<String>,

    #[serde(default = "default_verbosity")]
    pub verbosity: String,
}

impl Default for Otto {
    fn default() -> Self {
        default_otto()
    }
}
