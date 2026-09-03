//#![allow(unused_imports, unused_variables, dead_code)]

use eyre::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::vec::Vec;

/// The `otto.api` version this otto writes, and the one it assumes when an
/// ottofile declares no `api:` at all.
pub const CURRENT_API_VERSION: &str = "1";

/// Every `otto.api` version this otto has reviewed and executes correctly. An
/// ottofile declaring anything else is a loud failure in [`check_api_version`],
/// checked BEFORE the typed parse so the operator is told the file is from a
/// newer otto instead of being handed a confusing complaint about one key.
///
/// A SET, not a floor: a future `"2"` must be read and added deliberately,
/// rather than being accepted because it is numerically larger.
///
/// **Policy for growing the set.** A new version is added when, and only when,
/// otto makes a change that a prior otto would MIS-EXECUTE rather than merely
/// fail to understand. Adding an optional field does NOT bump it: strict
/// parsing already rejects the unknown key with a truthful message. Renaming or
/// re-typing an existing key, or changing what an existing key means, DOES. Old
/// versions stay in the set as long as otto still executes them correctly,
/// which is why this is a set and not a floor.
pub const SUPPORTED_API_VERSIONS: &[&str] = &[CURRENT_API_VERSION];

/// Minimal, deliberately tolerant view of an ottofile: just `otto.api`. Parsed
/// BEFORE the typed [`crate::cfg::config::ConfigSpec`] parse so a version
/// mismatch surfaces as "upgrade otto" instead of a confusing complaint about
/// whichever key the newer schema added.
///
/// It carries no `deny_unknown_fields` and every field is `Option` with a
/// default: the whole point is that it survives a document it does not
/// understand. A file with no `otto:` block, or an `otto:` block with no
/// `api:`, parses to `None` and is treated as [`CURRENT_API_VERSION`].
///
/// Deliberate deviation from borg (`borg/src/harvest/contract.rs:205-212`),
/// whose `VersionHeader.schema_version` is a required `u32`: borg emits its own
/// contracts and can require the field, while otto's ottofiles are hand-written
/// and `api:` is optional today. Requiring it would break them for no gain.
#[derive(Deserialize)]
struct ApiHeader {
    #[serde(default)]
    otto: Option<ApiHeaderOtto>,
}

#[derive(Deserialize)]
struct ApiHeaderOtto {
    #[serde(default)]
    api: Option<String>,
}

/// Reject an ottofile whose declared `otto.api` this otto does not speak.
///
/// Tolerant by construction: a document that does not even yield an
/// `ApiHeader` (unparseable YAML, an `otto:` block of the wrong shape) is
/// passed through so the typed parse can report the real, specific error.
pub fn check_api_version(content: &str) -> Result<()> {
    let Ok(header) = yaml_serde::from_str::<ApiHeader>(content) else {
        log::debug!("cfg::check_api_version: no readable api header, deferring to the typed parse");
        return Ok(());
    };
    let declared = header.otto.and_then(|otto| otto.api);
    log::debug!("cfg::check_api_version: declared={declared:?} supported={SUPPORTED_API_VERSIONS:?}");
    if let Some(api) = declared
        && !SUPPORTED_API_VERSIONS.contains(&api.as_str())
    {
        bail!(
            "otto: unsupported api version '{}' (this otto supports: {}). upgrade otto.",
            api,
            SUPPORTED_API_VERSIONS.join(", ")
        );
    }
    Ok(())
}

/// Wrap a strict-parse (`deny_unknown_fields`) failure with a trailing line
/// naming the likely cause and the fix, when the failure is an unknown-field
/// rejection (design doc `2026-09-01-cancellation-reaping-and-foreach-
/// concurrency.md`, Phase 4).
///
/// `check_api_version` already covers the case an ottofile can assert about
/// itself (`otto.api` names a generation this binary refuses). It cannot
/// cover the case here: an ottofile with no `api:` bump at all, using a key
/// this binary predates. `deny_unknown_fields` already names the key and its
/// path; what it cannot say is WHY the key is unknown, because serde has no
/// way to know. There are exactly two explanations and this otto cannot tell
/// them apart: the key is new, added by an otto released after this binary
/// (the pre-2.1.0 upgrade cliff, Problem Statement item 3), or the key is
/// simply misspelled in the ottofile. Both get the same next step named,
/// without asserting the first explanation as the only one: a genuinely
/// misspelled key (`tsaks:`) is not this binary's fault, and telling that
/// user their otto is out of date would be a wrong diagnosis dressed up as a
/// confident one.
///
/// **Deliberately NOT an api-version bump.** `SUPPORTED_API_VERSIONS` policy
/// (above) forbids growing the set for an additive key, and this wrapper is
/// the mechanism that covers additive keys instead: it needs no otto release
/// to have shipped a new generation, and it fires on every future addition
/// natively.
///
/// A no-op for every other `yaml_serde` failure (a missing field, a type
/// mismatch, unparseable YAML): none of those are "a key this binary does
/// not recognize", so none of them get the trailing line.
pub fn wrap_unknown_field_error(err: yaml_serde::Error) -> eyre::Report {
    let message = err.to_string();
    if message.contains("unknown field") {
        eyre::eyre!(
            "{message}\n\
             this key is either new to a newer otto than this binary, or simply \
             misspelled in the ottofile; if the ottofile targets a newer otto, run \
             `otto Upgrade` to update this binary"
        )
    } else {
        eyre::Report::new(err)
    }
}

fn default_name() -> String {
    "otto".to_string()
}

fn default_about() -> String {
    "A task runner".to_string()
}

fn default_api() -> String {
    CURRENT_API_VERSION.to_string()
}

fn default_tasks() -> Vec<String> {
    vec!["*".to_string()]
}

fn default_keep_days() -> u64 {
    30
}

fn default_keep_last() -> usize {
    10
}

fn default_keep_failed() -> u64 {
    60
}

fn default_auto_prune() -> bool {
    true
}

fn default_prune_interval_hours() -> u64 {
    24
}

/// `deny_unknown_fields` turns a stale or misplaced `otto.retention` key into
/// a loud config-load error naming the field, rather than a silently-ignored
/// no-op. Every field here is plain snake_case with no rename; `otto Convert`
/// emits exactly these names, so its own output stays loadable under this
/// attribute.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionSpec {
    /// Delete runs older than this many days (default: 30)
    #[serde(default = "default_keep_days")]
    pub keep_days: u64,

    /// Always keep at least this many most recent runs (default: 10)
    #[serde(default = "default_keep_last")]
    pub keep_last: usize,

    /// Keep failed runs for this many days (default: 60)
    #[serde(default = "default_keep_failed")]
    pub keep_failed: u64,

    /// Enable automatic pruning after runs (default: true)
    #[serde(default = "default_auto_prune")]
    pub auto_prune: bool,

    /// Minimum hours between auto-prune runs (default: 24)
    #[serde(default = "default_prune_interval_hours")]
    pub prune_interval_hours: u64,
}

impl RetentionSpec {
    /// True when every retention knob is still at its default, so the block
    /// can be omitted on serialize.
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl Default for RetentionSpec {
    fn default() -> Self {
        Self {
            keep_days: default_keep_days(),
            keep_last: default_keep_last(),
            keep_failed: default_keep_failed(),
            auto_prune: default_auto_prune(),
            prune_interval_hours: default_prune_interval_hours(),
        }
    }
}

#[must_use]
/// The default `otto:` block.
///
/// Delegates to `OttoSpec::default()` so the two cannot drift; they were
/// duplicated field-for-field, which meant adding a field to `OttoSpec`
/// silently left one of them wrong.
pub fn default_otto() -> OttoSpec {
    OttoSpec::default()
}

/// Reject `otto.jobs: 0` at config load.
///
/// `-j 0` is rejected by clap's `value_parser!(u64).range(1..)`, but the
/// ottofile path had no equivalent guard. This document's plan originally
/// DROPPED that validation on the recorded premise that "`otto.jobs` has zero
/// consumers, so `jobs: 0` in an ottofile runs fine". Both halves of that
/// premise later became false: `otto.jobs` is consumed at
/// `cli/parser.rs:777-779` whenever `-j` is absent, and `jobs: 0` then
/// reproduces the exact hot spin the `-j 0` fix removed - the launch loop's
/// `while active_tasks.len() < max_concurrent` never admits a task and the
/// sweep `continue`s forever at 100% CPU. `permits_for`'s
/// `debug_assert!(max_parallel >= 1)` never fires because the loop never
/// reaches it. Measured before this guard: `timeout 12s otto build` exited
/// 124 with zero output.
fn deserialize_jobs<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
where
    D: Deserializer<'de>,
{
    let jobs = Option::<usize>::deserialize(deserializer)?;
    if jobs == Some(0) {
        return Err(serde::de::Error::custom(
            "otto.jobs: 0 is not a valid job count; use 1 or more (omit the key to default to the CPU count)",
        ));
    }
    Ok(jobs)
}

// Serialization predicates. A field whose value still equals its own default
// is omitted, so round-tripping an ottofile that wrote no `otto:` block does
// not hand back a 17-line one. `api` has no predicate on purpose: it is the
// schema version and is always worth emitting.
fn is_default_name(v: &String) -> bool {
    *v == default_name()
}

fn is_default_about(v: &String) -> bool {
    *v == default_about()
}

// `default_tasks()` is `["*"]`, not `[]` - `Vec::is_empty` would leave the
// default value emitted on every partially-customized `otto:` block (e.g.
// one that sets only `jobs:`), which is exactly the null-noise this bullet
// exists to remove.
fn is_default_tasks(v: &[String]) -> bool {
    v == default_tasks()
}

/// `deny_unknown_fields` turns a stale or misplaced `otto:` key into a loud
/// config-load error naming the field, rather than a silently-ignored no-op.
/// Does not reach `envs`' free-form keys: the attribute governs `OttoSpec`'s
/// own field names, not the contents of the `HashMap` that `envs` holds.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OttoSpec {
    #[serde(default = "default_name", skip_serializing_if = "is_default_name")]
    pub name: String,

    #[serde(default = "default_about", skip_serializing_if = "is_default_about")]
    pub about: String,

    #[serde(default = "default_api")]
    pub api: String,

    /// Default parallelism, used only when `-j/--jobs` was not given
    /// explicitly on the command line (see `Parser::parse`'s `value_source`
    /// check). The CLI flag always wins when present.
    ///
    /// `None` means the ottofile did not set it, and the CPU count is resolved
    /// in the one place that owns the default (`cli::parser`'s `DEFAULT_JOBS`).
    /// It used to be a `usize` pre-filled with the host's CPU count and skipped
    /// on serialize when it still equalled that count, so `jobs: 4` on a 4-core
    /// host was dropped on re-emit and an ottofile that never wrote the key was
    /// indistinguishable from one that wrote the host's count.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_jobs"
    )]
    pub jobs: Option<usize>,

    #[serde(default = "default_tasks", skip_serializing_if = "is_default_tasks")]
    pub tasks: Vec<String>,

    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub envs: HashMap<String, String>,

    /// Command whose `KEY=VALUE` stdout LAYERS under `envs` as global
    /// environment variables. Kebab on disk, matching `on-failure` and
    /// `choices-command`. Resolved at most once per invocation, lazily, and
    /// never for `--help`; a literal `envs:` entry still wins its key.
    #[serde(default, rename = "envs-command", skip_serializing_if = "Option::is_none")]
    pub envs_command: Option<String>,

    #[serde(default, skip_serializing_if = "RetentionSpec::is_default")]
    pub retention: RetentionSpec,
}

impl Default for OttoSpec {
    fn default() -> Self {
        Self {
            name: default_name(),
            about: default_about(),
            api: default_api(),
            jobs: None,
            tasks: default_tasks(),
            envs: HashMap::new(),
            envs_command: None,
            retention: RetentionSpec::default(),
        }
    }
}

#[path = "otto_tests.rs"]
mod tests;
