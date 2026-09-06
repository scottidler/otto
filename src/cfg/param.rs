//#![allow(unused_imports, unused_variables, dead_code)]

use eyre::Result;
use indexmap::IndexMap;
use serde::de::{Deserializer, Error, MapAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::vec::Vec;

// IndexMap, not HashMap: same determinism rationale as TaskSpecs (see
// cfg/task.rs) - one param map serialized twice must emit its keys in the
// same author order both times.
pub type ParamSpecs = IndexMap<String, ParamSpec>;

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
/// This also makes the five `#[serde(skip)]` fields below (`name`, `short`,
/// `long`, `param_type`, `value`) newly REJECTED input rather than
/// silently-ignored input: writing e.g. `name:` under a param, which was
/// always a no-op because these fields are derived from the params-map key via
/// `divine()`, now errors. Verified: nothing in
/// this repo or the 159 external ottofiles under `~/repos` writes them.
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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metavar: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,

    // ParamSpec has no struct-level kebab rename, so the on-disk `choices-command`
    // key needs an explicit one here. It must apply to BOTH directions: a
    // deserialize-only rename would parse the ottofile and then re-emit
    // `choices_command`, which is the asymmetry tests/roundtrip.rs pins.
    #[serde(default, rename = "choices-command", skip_serializing_if = "Option::is_none")]
    pub choices_command: Option<String>,

    #[serde(default, skip_serializing_if = "Nargs::is_one")]
    pub nargs: Nargs,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,

    // clap enforces this (`arg.required(true)` in `param_to_arg`), with a
    // preflight ahead of it: `discovery.rs`'s clap bind gate never runs for a
    // task named with zero arguments, so an unadorned `required: true` alone
    // would not fire on the bare-invocation case this key exists for (design
    // doc `2026-08-31-buffered-foreach-computed-envs-required-params.md`,
    // Phase 1). Skipped on serialize when false so a plain param emits
    // nothing new.
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,

    // Runtime state, populated after CLI parsing — never part of the on-disk
    // ottofile representation.
    #[serde(skip)]
    pub value: Value,
}

/// `serde`'s `skip_serializing_if` needs a named function; `bool::not` doesn't
/// have the right signature (`&bool -> bool`, not `bool -> bool`), so a plain
/// closure can't be named in the attribute either.
fn is_false(value: &bool) -> bool {
    !*value
}

impl ParamSpec {
    /// True when the valid value set comes from a command's stdout rather than
    /// from the ottofile itself. A dynamic set executes, so it resolves lazily
    /// and only through the parser's `DynamicResolver`.
    #[must_use]
    pub fn has_dynamic_choices(&self) -> bool {
        self.choices_command.is_some()
    }

    /// Load-time rejections for `required: true`, checked before any
    /// subprocess runs so every rejection is shape-only (design doc Phase 1).
    ///
    /// Positional-after-optional-positional ordering is NOT checked here: it
    /// is a cross-param, declaration-order property of one task's whole
    /// params map, not a single param's own shape, so it is checked once per
    /// task by the caller (`Parser::validate_required_params`).
    pub fn validate_required(&self, task_name: &str) -> Result<()> {
        if !self.required {
            return Ok(());
        }
        if self.param_type == ParamType::FLG {
            return Err(eyre::eyre!(
                "Task '{task_name}' param '{}': `required: true` on a flag is meaningless; a \
                 required boolean that must always be passed is a constant",
                self.name
            ));
        }
        if self.default.is_some() {
            return Err(eyre::eyre!(
                "Task '{task_name}' param '{}': `required: true` cannot be combined with \
                 `default:`; a default makes required unreachable",
                self.name
            ));
        }
        let zero_capable = match self.nargs {
            Nargs::Zero => Some("0"),
            Nargs::OneOrZero => Some("?"),
            Nargs::ZeroOrMore => Some("*"),
            Nargs::One | Nargs::OneOrMore | Nargs::Range(..) => None,
        };
        if let Some(spelling) = zero_capable {
            return Err(eyre::eyre!(
                "Task '{task_name}' param '{}': `required: true` cannot be combined with \
                 `nargs: '{spelling}'`, which means the param may appear zero times",
                self.name
            ));
        }
        Ok(())
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

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Value {
    Item(String),
    List(Vec<String>),
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
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Item(s) => write!(f, "Value::Item({s})"),
            Self::List(l) => write!(f, "Value::List([{}])", l.join(", ")),
            Self::Empty => write!(f, "Value::Empty"),
        }
    }
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

impl Nargs {
    /// True for the implicit `nargs: '1'`, which is what a param that never
    /// mentions `nargs` deserializes to. Serialization skips it so a minimal
    /// param does not gain a key its ottofile never wrote.
    fn is_one(&self) -> bool {
        matches!(self, Self::One)
    }
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
            // `min == max` is what a bare `nargs: "N"` deserializes to, and it
            // re-emits as the bare form it was written as. Anything else is a
            // real span and emits `min:max`, both counts as the user wrote them.
            Self::Range(min, max) if min == max => min.to_string(),
            Self::Range(min, max) => format!("{min}:{max}"),
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
            Self::Range(min, max) => write!(formatter, "Nargs::Range[{min}, {max}]"),
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
                    if parts.len() != 2 {
                        return Err(Error::custom(format!(
                            "nargs '{s}': expected 'min:max' with exactly one ':', got {} parts",
                            parts.len()
                        )));
                    }
                    let min: usize = parts[0]
                        .parse()
                        .map_err(|e| Error::custom(format!("nargs '{s}': invalid min '{}': {e}", parts[0])))?;
                    let max: usize = parts[1]
                        .parse()
                        .map_err(|e| Error::custom(format!("nargs '{s}': invalid max '{}': {e}", parts[1])))?;
                    if min == 0 {
                        return Err(Error::custom(format!("nargs '{s}': min must be at least 1, got 0")));
                    }
                    if min > max {
                        return Err(Error::custom(format!(
                            "nargs '{s}': min ({min}) must not be greater than max ({max})"
                        )));
                    }
                    Self::Range(min, max)
                } else {
                    // A bare integer means EXACTLY N, which is what it means to
                    // clap (`num_args(N)`) and to argparse (`nargs=N`). It used
                    // to mean 1..=N in code and 0..=N in the reference page. A
                    // bounded zero-to-N is not expressible: use `?` for
                    // zero-or-one or `*` for zero-or-more.
                    let num = s
                        .parse()
                        .map_err(|e| Error::custom(format!("nargs '{s}': invalid count: {e}")))?;
                    Self::Range(num, num)
                }
            }
        };
        Ok(result)
    }
}

/// Divine a param's name/short/long form from its params-map key (e.g.
/// `-v|--verbose`).
///
/// Previously infallible and silently wrong on garbage input: `-v|-x` kept
/// only the first short and dropped `-x`; `--foo|--bar` concatenated into a
/// single bogus long `"foo--bar"`; `-verbose` (one dash, multiple letters -
/// almost certainly a typo for `--verbose`) became a *positional* param
/// literally named `-verbose`; and two keys divining to the same name (e.g.
/// `-v|--verbose` and `--verbose` in one map) silently collapsed to one
/// entry, the second's fields winning with no signal the first ever existed.
/// Every one of those is now a rejected config, naming the offending key.
fn divine(title: &str) -> Result<(String, Option<char>, Option<String>)> {
    let flags: Vec<&str> = title.split('|').collect();
    let mut short: Option<char> = None;
    let mut long: Option<String> = None;
    let mut bare: Option<&str> = None;

    for &flag in &flags {
        if flag.is_empty() {
            return Err(eyre::eyre!("param key '{title}': empty flag between '|'"));
        }
        if let Some(name) = flag.strip_prefix("--") {
            if name.is_empty() {
                return Err(eyre::eyre!("param key '{title}': '--' names no flag"));
            }
            if let Some(existing) = &long {
                return Err(eyre::eyre!(
                    "param key '{title}': two long flags ('--{existing}' and '{flag}'); a param \
                     takes at most one"
                ));
            }
            long = Some(name.to_string());
        } else if let Some(rest) = flag.strip_prefix('-') {
            let mut chars = rest.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return Err(eyre::eyre!(
                    "param key '{title}': '{flag}' is not a valid short flag (expected exactly \
                     one character after '-'; did you mean '-{flag}' with a second '-'?)"
                ));
            };
            if let Some(existing) = short {
                return Err(eyre::eyre!(
                    "param key '{title}': two short flags ('-{existing}' and '{flag}'); a param \
                     takes at most one"
                ));
            }
            short = Some(c);
        } else {
            if let Some(existing) = bare {
                return Err(eyre::eyre!(
                    "param key '{title}': multiple bare names ('{existing}' and '{flag}')"
                ));
            }
            bare = Some(flag);
        }
    }

    if let Some(name) = bare {
        if short.is_some() || long.is_some() {
            return Err(eyre::eyre!(
                "param key '{title}': '{name}' cannot be combined with a '-'/'--' flag in the \
                 same key; a positional param takes no flag form"
            ));
        }
        return Ok((name.to_string(), None, None));
    }

    let name = long
        .clone()
        .unwrap_or_else(|| short.map(|c| c.to_string()).unwrap_or_default());
    Ok((name, short, long))
}

pub fn deserialize_param_map<'de, D>(deserializer: D) -> Result<ParamSpecs, D::Error>
where
    D: Deserializer<'de>,
{
    log::debug!("cfg::deserialize_param_map: entering");
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
                let (name, short, long) = divine(&title).map_err(M::Error::custom)?;

                // Two keys that divine to the same name silently collapsed
                // into one entry (last-wins), losing the first param with no
                // signal it ever existed.
                if params.contains_key(&name) {
                    return Err(M::Error::custom(format!(
                        "param key '{title}': divines to name '{name}', which another param key \
                         in this map already divines to; param names must be unique"
                    )));
                }

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

#[path = "param_tests.rs"]
mod tests;
