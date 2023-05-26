//#![allow(unused_imports, unused_variables, dead_code)]

use eyre::Result;
use serde::de::{Deserializer, Error, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::vec::Vec;

pub type Params = HashMap<String, Param>;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Param {
    #[serde(skip_deserializing)]
    pub name: String,

    #[serde(skip_deserializing)]
    pub short: Option<char>,

    #[serde(skip_deserializing)]
    pub long: Option<String>,

    #[serde(skip_deserializing, default)]
    pub param_type: ParamType,

    #[serde(default)]
    pub dest: Option<String>,

    #[serde(default)]
    pub metavar: Option<String>,

    #[serde(default)]
    pub default: Option<String>,

    #[serde(default, deserialize_with = "deserialize_value")]
    pub constant: Value,

    #[serde(default)]
    pub choices: Vec<String>,

    #[serde(default)]
    pub nargs: Nargs,

    #[serde(default)]
    pub help: Option<String>,

    #[serde(skip_deserializing)]
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamType {
    FLG,
    OPT,
    POS,
}

impl Default for ParamType {
    fn default() -> Self {
        Self::OPT
    }
}

pub type Values = HashMap<String, Value>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Item(String),
    List(Vec<String>),
    Dict(HashMap<String, String>),
    Empty,
}

impl Default for Value {
    fn default() -> Self {
        Self::Empty
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Item(s) => write!(f, "Value::Item({s})"),
            Self::List(l) => write!(f, "Value::List([{}])", l.join(", ")),
            Self::Dict(d) => write!(
                f,
                "Value::Dict({{{}}})",
                d.iter()
                    .map(|(k, v)| format!("{k}: {v}"))
                    .collect::<Vec<String>>()
                    .join(", ")
            ),
            Self::Empty => write!(f, "Value::Empty"),
        }
    }
}

fn deserialize_value<'de, D>(deserializer: D) -> Result<Value, D::Error>
where
    D: Deserializer<'de>,
{
    struct ValueEnum;
    impl<'de> Visitor<'de> for ValueEnum {
        type Value = Value;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("string or list of strings")
        }
        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: Error,
        {
            Ok(Value::Item(value.to_owned()))
        }
        fn visit_seq<S>(self, mut visitor: S) -> Result<Self::Value, S::Error>
        where
            S: SeqAccess<'de>,
        {
            let mut vec: Vec<String> = vec![];
            while let Some(item) = visitor.next_element()? {
                vec.push(item);
            }
            Ok(Value::List(vec))
        }
    }
    deserializer.deserialize_any(ValueEnum)
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nargs {
    One,
    Zero,
    OneOrZero,
    OneOrMore,
    ZeroOrMore,
    Range(usize, usize),
}

impl Default for Nargs {
    fn default() -> Self {
        Self::One
    }
}

impl fmt::Display for Nargs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::One => write!(formatter, "Nargs::One[1]"),
            Self::Zero => write!(formatter, "Nargs::Zero[0]"),
            Self::OneOrZero => write!(formatter, "Nargs::OneOrZero[?]"),
            Self::OneOrMore => write!(formatter, "Nargs::OneOrMore[+]"),
            Self::ZeroOrMore => write!(formatter, "Nargs::ZeroOrMore[*]"),
            Self::Range(min, max) => write!(formatter, "Nargs::Range[{}, {}]", min + 1, max),
        }
    }
}

impl<'de> Deserialize<'de> for Nargs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let result = match &s[..] {
            "1" => Self::One,
            "0" => Self::Zero,
            "?" => Self::OneOrZero,
            "+" => Self::OneOrMore,
            "*" => Self::ZeroOrMore,
            _ => {
                println!("s={s}");
                if s.contains(':') {
                    let parts: Vec<&str> = s.split(':').collect();
                    let min: usize = parts[0].parse().map_err(Error::custom)?;
                    let max: usize = parts[1].parse().map_err(Error::custom)?;
                    Self::Range(min - 1, max)
                } else {
                    let num = s.parse().map_err(Error::custom)?;
                    Self::Range(0, num)
                }
            }
        };
        Ok(result)
    }
}

fn divine(title: &str) -> (String, Option<char>, Option<String>) {
    let flags: Vec<String> = title.split('|').map(std::string::ToString::to_string).collect();
    let short = flags
        .iter()
        .cloned()
        .filter(|i| i.starts_with('-') && i.len() == 2)
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .next();

    let long = Some(String::from(
        flags
            .iter()
            .cloned()
            .filter(|i| i.starts_with("--") && i.len() > 2)
            .collect::<String>()
            .trim_matches('-'),
    ))
    .filter(|s| !s.is_empty());

    //calculate the name to be long if exists, or short, or default to title
    let name = long
        .clone()
        .unwrap_or_else(|| short.map_or_else(|| title.to_string(), |c| c.to_string()));

    (name, short, long)
}

pub fn deserialize_param_map<'de, D>(deserializer: D) -> Result<Params, D::Error>
where
    D: Deserializer<'de>,
{
    struct ParamMap;

    impl<'de> Visitor<'de> for ParamMap {
        type Value = Params;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map of name to Param")
        }

        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut params = Params::new();
            while let Some((title, mut param)) = map.next_entry::<String, Param>()? {
                (param.name, param.short, param.long) = divine(&title);
                if param.long.is_some() || param.short.is_some() {
                    if let Some(ref value) = param.default {
                        if value == "true" || value == "false" {
                            param.param_type = ParamType::FLG;
                        }
                    }
                } else {
                    param.param_type = ParamType::POS;
                }
                params.insert(title.clone(), param);
            }
            Ok(params)
        }
    }
    deserializer.deserialize_map(ParamMap)
}
