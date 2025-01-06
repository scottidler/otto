// cfg/config.rs

use serde::Deserialize;

pub use crate::cfg::otto::{default_otto, Otto};
pub use crate::cfg::task::{deserialize_task_map, Task, Tasks};
pub use crate::cfg::param::{Param, Params, Value};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Config {
    #[serde(default = "default_otto")]
    pub otto: Otto,

    #[serde(default, deserialize_with = "deserialize_task_map")]
    pub tasks: Tasks,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            otto: default_otto(),
            tasks: Tasks::new(),
        }
    }
}
