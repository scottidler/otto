#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::error::ConfigError;
use super::param::Param;
use super::spec::{Spec, Task};

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

    pub fn load(&self) -> Result<Spec, ConfigError> {
        let content = fs::read_to_string(&self.ottofile)?;
        let spec: Spec = serde_yaml::from_str(&content)?;
        Ok(spec)
    }
}
