#![allow(unused_imports, unused_variables, unused_attributes, unused_mut, dead_code)]

use std::env;
use std::fmt::{Debug, Display, Formatter, Result as FmtResult};
use std::path::PathBuf;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum OttofileError {
    #[error("env var error: {0}")]
    HomeUndefined(#[from] env::VarError),
    #[error("canonicalize error")]
    CanoncalizeError(#[from] std::io::Error),
    #[error("divinie error; unable to find ottofile from path=[{0}]")]
    DivineError(PathBuf),
    #[error("relative path error")]
    RelativePathError,
    #[error("current exe filename error")]
    CurrentExeFilenameError,
    #[error("unknown error")]
    Unknown,
}

#[derive(Error, Debug)]
pub enum OttoParseError {
    #[error("config error")]
    ConfigError(#[from] crate::cfg::error::ConfigError),
    #[error("Clap parse error")]
    ClapError(#[from] clap::Error),
    #[error("unknown error")]
    Unknown,
}

#[derive(Error, Debug)]
pub struct SilentError;

impl Display for SilentError {
    fn fmt(&self, _f: &mut Formatter) -> FmtResult {
        Ok(())
    }
}