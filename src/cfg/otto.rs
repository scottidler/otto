//#![allow(unused_imports, unused_variables, dead_code)]

use serde::Deserialize;
use std::vec::Vec;

fn default_name() -> String {
    "otto".to_string()
}

fn default_home() -> String {
    "~/.otto".to_string()
}

fn default_about() -> String {
    "A task runner".to_string()
}

fn default_verbosity() -> String {
    "1".to_string()
}

fn default_api() -> String {
    "1".to_string()
}

fn default_jobs() -> usize {
    num_cpus::get()
}

fn default_tasks() -> Vec<String> {
    vec!["*".to_string()]
}

pub fn default_otto() -> Otto {
    Otto {
        name: default_name(),
        home: default_home(),
        about: default_about(),
        api: default_api(),
        verbosity: default_verbosity(),
        jobs: default_jobs(),
        tasks: default_tasks(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Otto {
    #[serde(default = "default_name")]
    pub name: String,

    #[serde(default = "default_home")]
    pub home: String,

    #[serde(default = "default_about")]
    pub about: String,

    #[serde(default = "default_api")]
    pub api: String,

    #[serde(default = "default_verbosity")]
    pub verbosity: String,

    #[serde(default = "default_jobs")]
    pub jobs: usize,

    #[serde(default = "default_tasks")]
    pub tasks: Vec<String>,
}

impl Default for Otto {
    fn default() -> Self {
        default_otto()
    }
}