//#![allow(unused_imports, unused_variables, dead_code)]

use eyre::Result;
use serde::de::{Deserializer, Error, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::vec::Vec;

pub type ParamSpecs = HashMap<String, ParamSpec>;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ParamSpec {
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

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize)]
pub enum ParamType {
    FLG,
    #[default]
    OPT,
    POS,
}

pub type Values = HashMap<String, Value>;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Value {
    Item(String),
    List(Vec<String>),
    Dict(HashMap<String, String>),
    #[default]
    Empty,
}

impl Serialize for Value {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Empty => serializer.serialize_none(),
            Self::Item(s) => serializer.serialize_str(s),
            Self::List(list) => {
                let mut seq = serializer.serialize_seq(Some(list.len()))?;
                for item in list {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Self::Dict(dict) => {
                let mut map = serializer.serialize_map(Some(dict.len()))?;
                for (k, v) in dict {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
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
            formatter.write_str("null, string, list of strings, or map of string to string")
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
        fn visit_map<M>(self, mut visitor: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut map: HashMap<String, String> = HashMap::new();
            while let Some((k, v)) = visitor.next_entry()? {
                map.insert(k, v);
            }
            Ok(Value::Dict(map))
        }
        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: Error,
        {
            Ok(Value::Empty)
        }
        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: Error,
        {
            Ok(Value::Empty)
        }
    }
    deserializer.deserialize_any(ValueEnum)
}
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Nargs {
    #[default]
    One,
    Zero,
    OneOrZero,
    OneOrMore,
    ZeroOrMore,
    Range(usize, usize),
}

impl Serialize for Nargs {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = match self {
            Self::One => "1".to_string(),
            Self::Zero => "0".to_string(),
            Self::OneOrZero => "?".to_string(),
            Self::OneOrMore => "+".to_string(),
            Self::ZeroOrMore => "*".to_string(),
            Self::Range(min, max) => format!("{}:{}", min + 1, max),
        };
        serializer.serialize_str(&s)
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
        .filter(|&i| i.starts_with('-') && i.len() == 2)
        .cloned()
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .next();

    let long = Some(String::from(
        flags
            .iter()
            .filter(|&i| i.starts_with("--") && i.len() > 2)
            .cloned()
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

pub fn deserialize_param_map<'de, D>(deserializer: D) -> Result<ParamSpecs, D::Error>
where
    D: Deserializer<'de>,
{
    struct ParamMap;

    impl<'de> Visitor<'de> for ParamMap {
        type Value = ParamSpecs;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map of name to Param")
        }

        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut params = ParamSpecs::new();
            while let Some((title, mut param_spec)) = map.next_entry::<String, ParamSpec>()? {
                let (name, short, long) = divine(&title);
                param_spec.name = name.clone();
                param_spec.short = short;
                param_spec.long = long;

                if param_spec.long.is_some() || param_spec.short.is_some() {
                    if let Some(ref value) = param_spec.default {
                        // Case-insensitive boolean detection
                        let lower_value = value.to_lowercase();
                        if lower_value == "true" || lower_value == "false" {
                            param_spec.param_type = ParamType::FLG;
                        }
                    }
                } else {
                    param_spec.param_type = ParamType::POS;
                }
                params.insert(name, param_spec);
            }
            Ok(params)
        }
    }
    deserializer.deserialize_map(ParamMap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_boolean_flag_detection_true_default() {
        use crate::cfg::task::TaskSpec;

        let yaml = r#"
        name: test_task
        params:
          -v|--verbose:
            default: true
            help: Enable verbose output
        "#;

        let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
        let verbose = task_spec.params.get("verbose").unwrap();

        assert_eq!(verbose.param_type, ParamType::FLG);
        assert_eq!(verbose.short, Some('v'));
        assert_eq!(verbose.long, Some("verbose".to_string()));
        assert_eq!(verbose.default, Some("true".to_string()));
        assert_eq!(verbose.name, "verbose");
    }

    #[test]
    fn test_boolean_flag_detection_false_default() {
        use crate::cfg::task::TaskSpec;

        let yaml = r#"
        name: test_task
        params:
          --debug:
            default: false
            help: Enable debug mode
        "#;

        let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
        let debug = task_spec.params.get("debug").unwrap();

        assert_eq!(debug.param_type, ParamType::FLG);
        assert_eq!(debug.short, None);
        assert_eq!(debug.long, Some("debug".to_string()));
        assert_eq!(debug.default, Some("false".to_string()));
        assert_eq!(debug.name, "debug");
    }

    #[test]
    fn test_boolean_flag_case_insensitive() {
        use crate::cfg::task::TaskSpec;

        let yaml = r#"
        name: test_task
        params:
          --enable:
            default: TRUE
            help: Enable feature
        "#;

        let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
        let enable = task_spec.params.get("enable").unwrap();

        assert_eq!(enable.param_type, ParamType::FLG);
        assert_eq!(enable.default, Some("TRUE".to_string()));
    }

    #[test]
    fn test_argument_flag_with_choices() {
        use crate::cfg::task::TaskSpec;

        let yaml = r#"
        name: test_task
        params:
          -e|--env:
            default: development
            choices: [development, staging, production]
            help: Target environment
        "#;

        let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
        let env = task_spec.params.get("env").unwrap();

        assert_eq!(env.param_type, ParamType::OPT);
        assert_eq!(env.short, Some('e'));
        assert_eq!(env.long, Some("env".to_string()));
        assert_eq!(env.choices, vec!["development", "staging", "production"]);
        assert_eq!(env.default, Some("development".to_string()));
        assert_eq!(env.name, "env");
    }

    #[test]
    fn test_argument_flag_no_default() {
        use crate::cfg::task::TaskSpec;

        let yaml = r#"
        name: test_task
        params:
          -c|--config:
            help: Path to config file
        "#;

        let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
        let config = task_spec.params.get("config").unwrap();

        assert_eq!(config.param_type, ParamType::OPT);
        assert_eq!(config.short, Some('c'));
        assert_eq!(config.long, Some("config".to_string()));
        assert_eq!(config.default, None);
        assert!(config.choices.is_empty());
    }

    #[test]
    fn test_positional_parameter() {
        use crate::cfg::task::TaskSpec;

        let yaml = r#"
        name: test_task
        params:
          filename:
            help: Input filename
        "#;

        let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
        let filename = task_spec.params.get("filename").unwrap();

        assert_eq!(filename.param_type, ParamType::POS);
        assert_eq!(filename.short, None);
        assert_eq!(filename.long, None);
        assert_eq!(filename.name, "filename");
    }

    #[test]
    fn test_positional_parameter_with_metavar() {
        use crate::cfg::task::TaskSpec;

        let yaml = r#"
        name: test_task
        params:
          input_file:
            help: Input file path
            metavar: FILE
        "#;

        let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
        let input_file = task_spec.params.get("input_file").unwrap();

        assert_eq!(input_file.param_type, ParamType::POS);
        assert_eq!(input_file.metavar, Some("FILE".to_string()));
    }

    #[test]
    fn test_mixed_parameters() {
        use crate::cfg::task::TaskSpec;

        let yaml = r#"
        name: test_task
        params:
          -v|--verbose:
            default: false
            help: Enable verbose output
          -e|--env:
            default: development
            choices: [development, staging, production]
            help: Target environment
          --timeout:
            default: 30
            help: Timeout in seconds
          input_file:
            help: Input file path
        "#;

        let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();

        // Boolean flag
        let verbose = task_spec.params.get("verbose").unwrap();
        assert_eq!(verbose.param_type, ParamType::FLG);
        assert_eq!(verbose.short, Some('v'));
        assert_eq!(verbose.long, Some("verbose".to_string()));

        // Argument flag with choices
        let env = task_spec.params.get("env").unwrap();
        assert_eq!(env.param_type, ParamType::OPT);
        assert_eq!(env.choices.len(), 3);

        // Argument flag without choices
        let timeout = task_spec.params.get("timeout").unwrap();
        assert_eq!(timeout.param_type, ParamType::OPT);
        assert!(timeout.choices.is_empty());

        // Positional parameter
        let input_file = task_spec.params.get("input_file").unwrap();
        assert_eq!(input_file.param_type, ParamType::POS);
    }

    #[test]
    fn test_divine_function_short_only() {
        let (name, short, long) = divine("-v");
        assert_eq!(name, "v");
        assert_eq!(short, Some('v'));
        assert_eq!(long, None);
    }

    #[test]
    fn test_divine_function_long_only() {
        let (name, short, long) = divine("--verbose");
        assert_eq!(name, "verbose");
        assert_eq!(short, None);
        assert_eq!(long, Some("verbose".to_string()));
    }

    #[test]
    fn test_divine_function_both() {
        let (name, short, long) = divine("-v|--verbose");
        assert_eq!(name, "verbose");
        assert_eq!(short, Some('v'));
        assert_eq!(long, Some("verbose".to_string()));
    }

    #[test]
    fn test_divine_function_reverse_order() {
        let (name, short, long) = divine("--verbose|-v");
        assert_eq!(name, "verbose");
        assert_eq!(short, Some('v'));
        assert_eq!(long, Some("verbose".to_string()));
    }

    #[test]
    fn test_divine_function_no_flags() {
        let (name, short, long) = divine("filename");
        assert_eq!(name, "filename");
        assert_eq!(short, None);
        assert_eq!(long, None);
    }

    #[test]
    fn test_value_display() {
        assert_eq!(Value::Empty.to_string(), "Value::Empty");
        assert_eq!(Value::Item("test".to_string()).to_string(), "Value::Item(test)");
        assert_eq!(
            Value::List(vec!["a".to_string(), "b".to_string()]).to_string(),
            "Value::List([a, b])"
        );

        let mut dict = HashMap::new();
        dict.insert("key".to_string(), "value".to_string());
        assert_eq!(Value::Dict(dict).to_string(), "Value::Dict({key: value})");
    }

    #[test]
    fn test_value_roundtrip_via_paramspec_constant() {
        // Round-trip Value through ParamSpec.constant, which uses deserialize_value.
        // This is the path that actually fires in production.
        let mut dict = HashMap::new();
        dict.insert("k1".to_string(), "v1".to_string());
        dict.insert("k2".to_string(), "v2".to_string());

        let cases = vec![
            Value::Empty,
            Value::Item("hello".to_string()),
            Value::List(vec!["a".to_string(), "b".to_string()]),
            Value::Dict(dict),
        ];

        for value in cases {
            let spec = ParamSpec {
                name: String::new(),
                short: None,
                long: None,
                param_type: ParamType::default(),
                dest: None,
                metavar: None,
                default: None,
                constant: value.clone(),
                choices: vec![],
                nargs: Nargs::default(),
                help: None,
                value: Value::Empty,
            };
            let yaml = serde_yaml::to_string(&spec).unwrap();
            let parsed: ParamSpec =
                serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("failed to parse {yaml:?}: {e}"));
            assert_eq!(
                spec.constant, parsed.constant,
                "constant round-trip failed for {value} (yaml: {yaml:?})"
            );
        }
    }

    #[test]
    fn test_nargs_roundtrip_all_variants() {
        let cases = vec![
            Nargs::One,
            Nargs::Zero,
            Nargs::OneOrZero,
            Nargs::OneOrMore,
            Nargs::ZeroOrMore,
            Nargs::Range(0, 3),
            Nargs::Range(2, 5),
        ];
        for nargs in cases {
            let yaml = serde_yaml::to_string(&nargs).unwrap();
            let parsed: Nargs = serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("failed to parse {yaml:?}: {e}"));
            assert_eq!(nargs, parsed, "round-trip failed for {nargs:?} (yaml: {yaml:?})");
        }
    }
}
