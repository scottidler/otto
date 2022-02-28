#![allow(unused_imports, unused_variables, dead_code)]
use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum ConfigError {
    FlagLookupError(String),
    NameLookupError(String),
}

impl Error for ConfigError {
    fn description(&self) -> &str {
        match *self {
            ConfigError::FlagLookupError(ref flag) => "flag lookup error",
            ConfigError::NameLookupError(ref name) => "name lookup error",
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ConfigError::FlagLookupError(ref flag) => {
                write!(fmtr, "flag lookup error; flag={} not found", flag)
            }
            ConfigError::NameLookupError(ref name) => {
                write!(fmtr, "name lookup error; name={} not found", name)
            }
        }
    }
}
