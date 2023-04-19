#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use eyre::Result;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::cfg::error::ConfigError;
use crate::cfg::spec::{ParamSpec, OttofileSpec, TaskSpec};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Loader {
    ottofile: PathBuf,
}

impl Loader {
    #[must_use]
    pub fn new(ottofile: &Path) -> Self {
        Self {
            ottofile: ottofile.to_path_buf(),
        }
    }

    pub fn load(&self) -> Result<OttofileSpec> {
        let content = fs::read_to_string(&self.ottofile)?;
        let spec: OttofileSpec = serde_yaml::from_str(&content)?;
        Ok(spec)
    }
}
