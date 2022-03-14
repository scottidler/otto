#![allow(unused_imports, unused_variables, dead_code)]
use anyhow::{Context, Error, Result};
use std::fs;
use std::path::{Path, PathBuf};

use super::spec::{Param, Spec, Task};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Loader {
    ottofile: PathBuf,
}

impl Loader {
    pub fn new(ottofile: &Path) -> Self {
        Self {
            ottofile: ottofile.to_path_buf(),
        }
    }

    //pub fn load(&self, filename: &str) -> Result<Spec, Error> {
    pub fn load(&self) -> Result<Spec, Error> {
        let content = fs::read_to_string(&self.ottofile)
            .context(format!("Can't load ottofile={:?}", self.ottofile))?;
        let spec: Spec =
            serde_yaml::from_str(&content).context(format!("Can't load content={:?}", content))?;
        Ok(spec)
    }
}
