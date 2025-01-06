// cfg/error.rs

//use std::fmt;
//use std::io;

//use std::fmt::{Debug, Display, Formatter};
use std::fmt::Debug;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config load error: {0}")]
    ConfigLoadError(#[from] std::io::Error),
    #[error("serde yaml error: {0}")]
    SerdeYamlError(#[from] serde_yaml::Error),
    /*
    #[error("flag lookup error; flag={0} not found")]
    FlagLookupError(String),
    #[error("name lookup error; name={0} not found")]
    NameLookupError(String),
    */
}

/*
impl Error for ConfigError {
    fn description(&self) -> &str {
        match *self {
            ConfigError::FlagLookupError(ref flag) => "flag lookup error",
            ConfigError::NameLookupError(ref name) => "name lookup error",
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ConfigError::FlagLookupError(ref flag) => {
                write!(formatter, "flag lookup error; flag={} not found", flag)
            }
            ConfigError::NameLookupError(ref name) => {
                write!(formatter, "name lookup error; name={} not found", name)
            }
        }
    }
}
*/
