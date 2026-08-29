//#![allow(unused_imports, unused_variables, dead_code)]

use eyre::Result;
use serde::de::{Deserializer, Error, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::vec::Vec;

pub type ParamSpecs = HashMap<String, ParamSpec>;

/// Reconstruct the rich params-map key (e.g. "-v|--verbose") from a ParamSpec's
/// derived fields. Inverse of `divine` for serialize-side emission.
fn rich_key(spec: &ParamSpec) -> String {
    match (spec.short, spec.long.as_deref()) {
        (Some(s), Some(l)) => format!("-{s}|--{l}"),
        (Some(s), None) => format!("-{s}"),
        (None, Some(l)) => format!("--{l}"),
        (None, None) => spec.name.clone(),
    }
}

/// Serializes a `ParamSpecs` map using the rich-key form for each entry's key.
/// The map's stored key (the divined name) is discarded on serialize because
/// it would round-trip lossily — see `docs/design/2026-05-24-paramspec-roundtrip.md`.
pub struct ParamMapSerializer<'a>(pub &'a ParamSpecs);

impl Serialize for ParamMapSerializer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for spec in self.0.values() {
            map.serialize_entry(&rich_key(spec), spec)?;
        }
        map.end()
    }
}

/// `deny_unknown_fields` turns a stale or misplaced param key into a loud
/// config-load error naming the field, rather than a silently-ignored no-op.
/// Per `borg/src/config.rs:281-285`. This also makes the five `#[serde(skip)]`
/// fields below (`name`, `short`, `long`, `param_type`, `value`) newly
/// REJECTED input rather than silently-ignored input: writing e.g. `name:`
/// under a param, which was always a no-op because these fields are derived
/// from the params-map key via `divine()`, now errors. Verified: nothing in
/// this repo or the 159 external ottofiles under `~/repos` writes them. Does
/// not reach `constant`'s free-form map: the attribute governs `ParamSpec`'s
/// own field names, not the contents of `Value`'s hand-written `visit_map`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    // INVARIANT: name/short/long/param_type are derived from the params-map
    // KEY (e.g. "-v|--verbose") by deserialize_param_map via divine(), and
    // are reconstructed back into the rich key on serialize by
    // ParamMapSerializer via rich_key(). divine() and rich_key() MUST remain
    // inverses for the round-trip to hold. Do not mutate these fields after
    // parse — either change the map key and re-divine, or the next serialize
    // will emit YAML that no longer matches the original input form. The
    // contract is locked in by tests/roundtrip.rs (ConfigSpec level) and
    // rich_key_reflects_current_fields below (rich_key unit level).
    //
    // See docs/design/2026-05-24-paramspec-roundtrip.md for context and the
    // criteria under which Option D (eliminate these fields entirely) would
    // become worth doing.
    #[serde(skip)]
    pub name: String,

    #[serde(skip)]
    pub short: Option<char>,

    #[serde(skip)]
    pub long: Option<String>,

    #[serde(skip)]
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

    // ParamSpec has no struct-level kebab rename, so the on-disk `choices-command`
    // key needs an explicit one here. It must apply to BOTH directions: a
    // deserialize-only rename would parse the ottofile and then re-emit
    // `choices_command`, which is the asymmetry tests/roundtrip.rs pins.
    #[serde(default, rename = "choices-command")]
    pub choices_command: Option<String>,

    #[serde(default)]
    pub nargs: Nargs,

    #[serde(default)]
    pub help: Option<String>,

    // Runtime state, populated after CLI parsing — never part of the on-disk
    // ottofile representation.
    #[serde(skip)]
    pub value: Value,
}

impl ParamSpec {
    /// True when the valid value set comes from a command's stdout rather than
    /// from the ottofile itself. A dynamic set executes, so it resolves lazily
    /// and only through the parser's `DynamicResolver`.
    #[must_use]
    pub fn has_dynamic_choices(&self) -> bool {
        self.choices_command.is_some()
    }

    /// Resolve a `choices-command:` param source: run the command and turn its
    /// non-empty stdout lines into the allowed value set.
    ///
    /// Contract (design doc Phase 6b), identical to `foreach: command:`: `sh -c`,
    /// cwd = the ottofile's directory, env = inherited environment + `envs` (the
    /// resolved global `envs:`); task params are NOT available. A non-zero exit
    /// is a loud error naming the task, the param, and the command.
    ///
    /// Zero lines is ALSO a loud error, deliberately unlike foreach's legitimate
    /// empty scope: a param whose valid set is empty can accept no value at all,
    /// which is a misconfiguration, and fail-closed beats accept-anything.
    pub fn resolve_choices_command(
        &self,
        task_name: &str,
        cwd: &std::path::Path,
        envs: &HashMap<String, String>,
    ) -> Result<Vec<String>> {
        let command = self.choices_command.as_ref().ok_or_else(|| {
            eyre::eyre!(
                "Task '{}' param '{}': no choices-command to resolve",
                task_name,
                self.name
            )
        })?;

        let key = format!("{task_name}:{}", self.name);
        let context = format!("Task '{task_name}' param '{}' choices-command", self.name);
        let values = crate::cfg::resolver::run_lines_command(
            command,
            cwd,
            envs,
            crate::cfg::resolver::CHOICES_GUARD_VAR,
            &key,
            &context,
        )?;

        if values.is_empty() {
            return Err(eyre::eyre!(
                "{context}: command '{command}' produced no values; a param whose \
                 valid set is empty can never be given a value"
            ));
        }

        Ok(values)
    }
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

                // Checked here, after divine(), so the error can name the param
                // by the identity the user will recognize. A static list and a
                // command are two answers to "what is valid?"; picking one
                // silently is exactly the kind of guess that hides a typo.
                if let Some(command) = &param_spec.choices_command
                    && !param_spec.choices.is_empty()
                {
                    return Err(M::Error::custom(format!(
                        "param '{name}': choices-command '{command}' cannot be combined with \
                         choices [{}]; a param takes exactly one source of valid values",
                        param_spec.choices.join(", ")
                    )));
                }

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

    /// Documents the contract that rich_key() is a *pure function of the
    /// current ParamSpec fields*, not a cached value of the input key. If a
    /// future refactor caches divine()'s input on the struct, this test
    /// breaks — that's the signal that the divine/rich_key inverse-pair
    /// invariant has been compromised and Option D (see design doc) should
    /// be revisited.
    #[test]
    fn rich_key_reflects_current_fields() {
        let mut spec = ParamSpec {
            name: "verbose".to_string(),
            short: Some('v'),
            long: Some("verbose".to_string()),
            param_type: ParamType::FLG,
            dest: None,
            metavar: None,
            default: Some("false".to_string()),
            constant: Value::Empty,
            choices_command: None,
            choices: vec![],
            nargs: Nargs::default(),
            help: None,
            value: Value::Empty,
        };
        assert_eq!(rich_key(&spec), "-v|--verbose");

        spec.long = Some("rename".to_string());
        assert_eq!(rich_key(&spec), "-v|--rename");

        spec.short = None;
        assert_eq!(rich_key(&spec), "--rename");

        spec.long = None;
        spec.name = "positional".to_string();
        assert_eq!(rich_key(&spec), "positional");
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

    /// Phase 4 negative test (design doc 2026-08-29, Phase 4 table). `name:`
    /// under a param is a `#[serde(skip)]` field, derived from the params-map
    /// key by `divine()`, never a user-writable key. Before this phase it was
    /// a silent no-op; now it is REJECTED input, naming the field and the
    /// path down to the rich param key.
    #[test]
    fn deny_unknown_fields_rejects_a_skip_field_written_by_the_user() {
        use crate::cfg::config::ConfigSpec;
        let yaml = "tasks:\n  up:\n    params:\n      -s|--svc:\n        name: something\n    bash: echo hi\n";
        let err = serde_yaml::from_str::<ConfigSpec>(yaml).unwrap_err().to_string();
        assert!(err.contains("name"), "must name the field: {err}");
        assert!(err.contains("tasks.up.params"), "must name the path: {err}");
        assert!(err.contains("-s|--svc"), "must name the rich param key: {err}");
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
                choices_command: None,
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

    // =========================================================================
    // choices-command (design doc Phase 6b)
    // =========================================================================

    fn spec_for(yaml: &str, param: &str) -> ParamSpec {
        use crate::cfg::task::TaskSpec;
        let task_spec: TaskSpec = serde_yaml::from_str(yaml).unwrap();
        task_spec.params.get(param).unwrap().clone()
    }

    #[test]
    fn choices_command_parses_from_the_kebab_case_key() {
        let spec = spec_for(
            r#"
            params:
              -s|--svc:
                choices-command: "printf 'alpha\nbeta\n'"
                help: Service to switch to
            "#,
            "svc",
        );
        assert_eq!(spec.choices_command.as_deref(), Some("printf 'alpha\nbeta\n'"));
        assert!(spec.has_dynamic_choices());
        assert!(spec.choices.is_empty());
    }

    #[test]
    fn choices_command_serializes_back_to_the_kebab_case_key() {
        // A deserialize-only rename would emit `choices_command` here, and the
        // ottofile would silently lose the field on the next parse.
        let spec = spec_for(
            r#"
            params:
              -s|--svc:
                choices-command: "list-services"
            "#,
            "svc",
        );
        let emitted = serde_yaml::to_string(&spec).unwrap();
        assert!(emitted.contains("choices-command: list-services"), "{emitted}");
        assert!(!emitted.contains("choices_command"), "{emitted}");
    }

    #[test]
    fn a_param_without_choices_command_stays_static() {
        let spec = spec_for(
            r#"
            params:
              -e|--env:
                choices: [dev, prod]
            "#,
            "env",
        );
        assert!(!spec.has_dynamic_choices());
        assert_eq!(spec.choices, vec!["dev".to_string(), "prod".to_string()]);
    }

    #[test]
    fn choices_and_choices_command_together_is_a_loud_config_error() {
        use crate::cfg::task::TaskSpec;
        let err = serde_yaml::from_str::<TaskSpec>(
            r#"
            params:
              -s|--svc:
                choices: [alpha, beta]
                choices-command: "list-services"
            "#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("svc"), "must name the param: {err}");
        assert!(err.contains("list-services"), "must name the command: {err}");
        assert!(err.contains("alpha, beta"), "must name the static choices: {err}");
    }

    #[test]
    fn resolve_choices_command_returns_trimmed_non_empty_lines() {
        let spec = spec_for(
            r#"
            params:
              -s|--svc:
                choices-command: "printf 'alpha\n\n  beta  \n'"
            "#,
            "svc",
        );
        let values = spec
            .resolve_choices_command("switch", std::path::Path::new("."), &HashMap::new())
            .unwrap();
        assert_eq!(values, vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn resolve_choices_command_nonzero_exit_names_task_param_and_command() {
        let spec = spec_for(
            r#"
            params:
              -s|--svc:
                choices-command: "echo nope >&2; exit 4"
            "#,
            "svc",
        );
        let err = spec
            .resolve_choices_command("switch", std::path::Path::new("."), &HashMap::new())
            .unwrap_err()
            .to_string();
        assert!(err.contains("switch"), "{err}");
        assert!(err.contains("svc"), "{err}");
        assert!(err.contains("echo nope >&2; exit 4"), "{err}");
        assert!(err.contains("exit code 4"), "{err}");
    }

    #[test]
    fn resolve_choices_command_zero_lines_is_an_error_unlike_foreach() {
        let spec = spec_for(
            r#"
            params:
              -s|--svc:
                choices-command: "true"
            "#,
            "svc",
        );
        let err = spec
            .resolve_choices_command("switch", std::path::Path::new("."), &HashMap::new())
            .unwrap_err()
            .to_string();
        assert!(err.contains("switch"), "{err}");
        assert!(err.contains("svc"), "{err}");
        assert!(err.contains("no values"), "{err}");
    }
}
