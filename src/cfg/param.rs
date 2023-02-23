use serde::de::{Deserializer, Error, SeqAccess, Visitor};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::vec::Vec;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamType {
    FLG,
    OPT,
    POS,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Item(String),
    List(Vec<String>),
    Dict(HashMap<String, String>),
    Empty,
}

impl fmt::Display for Value {
    fn fmt(&self, fmtr: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Item(s) => write!(fmtr, "Values::Item({s})"),
            Self::List(vs) => write!(fmtr, "Values::List([{}])", vs.join(", ")),
            Self::Dict(_ds) => write!(fmtr, "Values::Dict[NOT IMPLEMENTED]"),
            Self::Empty => write!(fmtr, "Value::Empty(\"\")"),
        }
    }
}

impl Default for Value {
    fn default() -> Self {
        Self::Empty
    }
}

fn deserialize_value<'de, D>(deserializer: D) -> Result<Value, D::Error>
where
    D: Deserializer<'de>,
{
    struct ValueEnum;
    impl<'de> Visitor<'de> for ValueEnum {
        type Value = Value;

        fn expecting(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
            fmtr.write_str("string or list of strings")
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

impl fmt::Display for Nargs {
    fn fmt(&self, fmtr: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::One => write!(fmtr, "Nargs::One[1]"),
            Self::Zero => write!(fmtr, "Nargs::Zero[0]"),
            Self::OneOrZero => write!(fmtr, "Nargs::OneOrZero[?]"),
            Self::OneOrMore => write!(fmtr, "Nargs::OneOrMore[+]"),
            Self::ZeroOrMore => write!(fmtr, "Nargs::ZeroOrMore[*]"),
            Self::Range(min, max) => write!(fmtr, "Nargs::Range[{}, {}]", min + 1, max),
        }
    }
}

impl Default for Nargs {
    fn default() -> Self {
        Self::One
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
                    let min = parts[0].parse::<usize>().map_err(Error::custom)?;
                    let max = parts[1].parse::<usize>().map_err(Error::custom)?;
                    Self::Range(min - 1, max)
                } else {
                    let num = s.parse::<usize>().map_err(Error::custom)?;
                    Self::Range(0, num)
                }
            }
        };
        Ok(result)
    }
}

pub type Params = HashMap<String, Param>;

// FIXME: Flag, Named and Positional Args
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
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
}
