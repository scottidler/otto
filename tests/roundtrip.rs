//! Round-trip property tests: parse a YAML config, re-emit, re-parse,
//! assert structural equality.
//!
//! This guards against `Serialize` / `Deserialize` asymmetries on config
//! types. Each new variant added to `Nargs`, `Value`, `When`, or any
//! ConfigSpec-reachable enum must round-trip cleanly here.

use otto::cfg::config::ConfigSpec;

fn assert_roundtrips(yaml: &str) {
    let config: ConfigSpec =
        serde_yaml_ng::from_str(yaml).unwrap_or_else(|e| panic!("initial parse failed: {e}\nyaml:\n{yaml}"));
    let emitted = serde_yaml_ng::to_string(&config).unwrap_or_else(|e| panic!("serialize failed: {e}"));
    let reparsed: ConfigSpec =
        serde_yaml_ng::from_str(&emitted).unwrap_or_else(|e| panic!("reparse failed: {e}\nemitted:\n{emitted}"));
    assert_eq!(
        config, reparsed,
        "structural drift after round-trip.\noriginal yaml:\n{yaml}\nemitted yaml:\n{emitted}"
    );
}

#[test]
fn config_otto_only_roundtrips() {
    assert_roundtrips(
        r#"
otto:
  jobs: 4
"#,
    );
}

/// `otto.jobs` used to be a `usize` pre-filled with the host's CPU count and
/// skipped on serialize while it still equalled that count, so this config
/// round-tripped on a 4-core host only because `jobs: 4` was dropped on the way
/// out and re-defaulted to 4 on the way back in. As an `Option`, a written key
/// is a written key on every host.
#[test]
fn jobs_equal_to_the_host_cpu_count_survives_the_round_trip() {
    let cpus = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let yaml = format!("otto:\n  jobs: {cpus}\n");
    assert_roundtrips(&yaml);

    let config: ConfigSpec = serde_yaml_ng::from_str(&yaml).expect("parse");
    let emitted = serde_yaml_ng::to_string(&config).expect("serialize");

    assert!(
        emitted.contains(&format!("jobs: {cpus}")),
        "the host's own CPU count must not be dropped;\ngot:\n{emitted}"
    );
}

/// A config that never wrote `jobs:` must not gain one: the CPU-count default
/// belongs to the CLI, not to the on-disk representation.
#[test]
fn an_absent_jobs_key_stays_absent_on_re_emit() {
    let yaml = "tasks:\n  build:\n    bash: echo hi\n";

    let config: ConfigSpec = serde_yaml_ng::from_str(yaml).expect("parse");
    let emitted = serde_yaml_ng::to_string(&config).expect("serialize");

    assert_eq!(emitted, yaml, "an absent jobs key must not be invented");
}

/// A bare `nargs: "N"` means exactly N and re-emits as the bare form it was
/// written as. It used to deserialize to `Range(0, N)` and re-emit as `"1:N"`,
/// so the spelling changed under the author on every re-emit.
#[test]
fn a_bare_nargs_count_round_trips_byte_identical() {
    let yaml = "tasks:\n  build:\n    params:\n      --files:\n        nargs: '3'\n    bash: echo hi\n";

    let config: ConfigSpec = serde_yaml_ng::from_str(yaml).expect("parse");
    let emitted = serde_yaml_ng::to_string(&config).expect("serialize");

    assert_eq!(emitted, yaml, "round-trip must be byte-identical");
}

/// A `min:max` span keeps the counts the author wrote rather than an
/// internally offset `min`: `"2:5"` used to deserialize to `Range(1, 5)` and
/// re-emit as `"2:5"` only because the serializer added the one back.
#[test]
fn an_nargs_span_re_emits_the_counts_that_were_written() {
    let yaml = "tasks:\n  build:\n    params:\n      --files:\n        nargs: '2:5'\n    bash: echo hi\n";
    assert_roundtrips(yaml);

    let config: ConfigSpec = serde_yaml_ng::from_str(yaml).expect("parse");
    let emitted = serde_yaml_ng::to_string(&config).expect("serialize");

    assert!(emitted.contains("nargs: 2:5"), "got:\n{emitted}");
    assert!(!emitted.contains("1:5"), "got:\n{emitted}");
}

/// A minimal `foreach:` used to emit seven keys its ottofile never wrote
/// (`glob: null`, `range: null`, `command: null`, `as: item`, `parallel:
/// true`, `max_items: 1000`, `buffer: false`). `ForeachSpec` now skips its
/// defaults the way `ParamSpec` does.
#[test]
fn a_minimal_foreach_round_trips_without_gaining_keys() {
    let yaml = "tasks:\n  up:\n    foreach:\n      items:\n      - a\n    bash: echo hi\n";

    let config: ConfigSpec = serde_yaml_ng::from_str(yaml).expect("parse");
    let emitted = serde_yaml_ng::to_string(&config).expect("serialize");

    assert_eq!(emitted, yaml, "round-trip must be byte-identical");
    for noise in [
        "glob:",
        "range:",
        "command:",
        "as:",
        "parallel:",
        "max_items:",
        "buffer:",
    ] {
        assert!(
            !emitted.contains(noise),
            "serialized form must not contain {noise:?};\ngot:\n{emitted}"
        );
    }
}

#[test]
fn config_with_tasks_no_params_roundtrips() {
    assert_roundtrips(
        r#"
tasks:
  build:
    action: cargo build
  test:
    action: cargo test
    before:
      - build
  lint:
    action: cargo clippy
"#,
    );
}

#[test]
fn config_with_conditional_edges_roundtrips() {
    assert_roundtrips(
        r#"
tasks:
  build:
    action: cargo build
  test:
    action: cargo test
    after:
      - task: build
        when: success
  cleanup:
    action: cargo clean
    after:
      - task: build
        when: failure
  notify:
    action: echo done
    after:
      - task: test
        when: always
"#,
    );
}

#[test]
fn config_with_foreach_roundtrips() {
    assert_roundtrips(
        r#"
tasks:
  run-all:
    action: ./bin/test.sh ${item}
    foreach:
      glob: "tests/*.sh"
      as: item
      parallel: true
"#,
    );
}

/// A `command:` foreach source must survive serialize -> re-parse verbatim,
/// the same as the three static sources (design doc 2026-08-28, Phase 6).
#[test]
fn config_with_foreach_command_roundtrips() {
    assert_roundtrips(
        r#"
tasks:
  up:
    action: ./bin/svc.sh ${svc} up
    foreach:
      command: "scripts/list-services.sh"
      as: svc
      parallel: false
"#,
    );
}

#[test]
fn config_with_io_specs_roundtrips() {
    assert_roundtrips(
        r#"
tasks:
  compile:
    action: cargo build
    input:
      - src/**/*.rs
      - Cargo.toml
    output:
      - target/release/otto
    envs:
      RUST_LOG: info
      CARGO_TERM_COLOR: always
"#,
    );
}

/// Exercises ParamMapSerializer's rich-key reconstruction. Before the fix in
/// `docs/design/2026-05-24-paramspec-roundtrip.md`, this case lost the
/// `-v|--verbose` form on serialize and decayed FLG/OPT params to POS on
/// re-parse.
#[test]
fn config_with_params_roundtrips() {
    assert_roundtrips(
        r#"
tasks:
  test:
    action: cargo test
    params:
      -v|--verbose:
        default: false
        help: Enable verbose output
      -e|--env:
        default: development
        choices: [development, staging, production]
        help: Target environment
      --timeout:
        default: "30"
        help: Timeout in seconds
      filename:
        help: Input file
"#,
    );
}

/// Phase 6b: a `choices-command:` param must re-serialize under its own kebab
/// name. ParamSpec has no struct-level rename, so a deserialize-only
/// `#[serde(rename)]` would parse this fine and then emit `choices_command`,
/// which the reparse would drop back to `None` - structural drift this catches.
#[test]
fn config_with_choices_command_roundtrips() {
    assert_roundtrips(
        r#"
tasks:
  switch:
    action: echo "switching to ${svc}"
    params:
      -s|--svc:
        choices-command: "printf 'alpha\nbeta\n'"
        help: Service to switch to
      -e|--env:
        choices: [dev, prod]
        help: Static sibling, must be unaffected
"#,
    );
}

/// The round-trip above proves the value survives; this proves the *spelling*
/// does, which is what a human reading the re-emitted ottofile depends on.
#[test]
fn choices_command_emits_its_kebab_case_key_verbatim() {
    let yaml = r#"
tasks:
  switch:
    action: echo hi
    params:
      -s|--svc:
        choices-command: "list-services --all"
"#;
    let config: ConfigSpec = serde_yaml_ng::from_str(yaml).expect("parse failed");
    let emitted = serde_yaml_ng::to_string(&config).expect("serialize failed");
    assert!(
        emitted.contains("choices-command: list-services --all"),
        "emitted yaml lost the kebab-case key:\n{emitted}"
    );
    assert!(!emitted.contains("choices_command"), "emitted snake_case:\n{emitted}");
}

/// Phase 7: `tty: true` must survive the round-trip. `TaskSpec` has a hand-written
/// `Serialize`, so a field added to the struct and the deserialize helper but not
/// to the serializer would parse fine and then vanish - the task would silently
/// lose its terminal on any re-emit.
#[test]
fn config_with_tty_task_roundtrips() {
    assert_roundtrips(
        r#"
tasks:
  login:
    action: aws sso login
    tty: true
  build:
    action: cargo build
  quiet:
    action: echo quiet
    tty: false
"#,
    );
}

/// Phase 6: `tests/roundtrip.rs` had no `#!` anywhere, so this invariant was
/// never tested for shebangs at all. A prefix match on serialize
/// (`starts_with("#!/bin/bash")`) used to also match a shebang WITH ARGS,
/// stripping only the "#!/bin/bash" substring and stranding the args on a
/// mangled continuation line that reparsed to a different action.
#[test]
fn config_with_shebang_args_roundtrips() {
    assert_roundtrips(
        r#"
tasks:
  build:
    bash: |
      #!/bin/bash -euo pipefail
      echo hello
"#,
    );
}

/// A NON-bash shebang with args round-trips too.
///
/// The bug was a prefix match on `"#!/bin/bash"`, so the shipped case pins the
/// one interpreter the buggy code special-cased. An interpreter it never matched
/// exercises the other side of the same branch. Added by the batched audit,
/// batch 7 of 14.
#[test]
fn config_with_non_bash_shebang_args_roundtrips() {
    assert_roundtrips(
        r#"
tasks:
  build:
    action: |
      #!/usr/bin/env python3 -u
      print("hello")
"#,
    );
}

/// A bare auto-added shebang (no args) is still recognized as `bash:` sugar
/// and stays terse on re-emit rather than falling back to `action:`.
#[test]
fn config_with_bash_sugar_emits_bash_key_not_action() {
    let yaml = r#"
tasks:
  build:
    bash: echo hello
"#;
    let config: ConfigSpec = serde_yaml_ng::from_str(yaml).expect("parse failed");
    let emitted = serde_yaml_ng::to_string(&config).expect("serialize failed");
    assert!(emitted.contains("bash:"), "{emitted}");
    assert!(!emitted.contains("action:"), "{emitted}");
}

/// `TaskSpecs`/`ParamSpecs` were `HashMap`-backed, so serializing one config
/// gave a different key order on (almost) every run - five serializes of one
/// 5-task config reproduced five distinct orders. `IndexMap` preserves
/// author order, so every serialize of the same parsed config must agree.
#[test]
fn five_serializes_of_one_config_produce_one_order() {
    let yaml = r#"
tasks:
  echo1:
    action: echo one
  echo2:
    action: echo two
    params:
      -a|--alpha:
        help: a
      -b|--beta:
        help: b
      -c|--gamma:
        help: c
  echo3:
    action: echo three
  echo4:
    action: echo four
  echo5:
    action: echo five
"#;
    // Re-parse from source on every iteration - a fresh `IndexMap` per parse,
    // not one map serialized five times - so this exercises "same insertions
    // produce the same order", not merely "one object serializes the same as
    // itself".
    let first = {
        let config: ConfigSpec = serde_yaml_ng::from_str(yaml).expect("parse failed");
        serde_yaml_ng::to_string(&config).expect("serialize failed")
    };
    for attempt in 1..5 {
        let config: ConfigSpec = serde_yaml_ng::from_str(yaml).expect("parse failed");
        let emitted = serde_yaml_ng::to_string(&config).expect("serialize failed");
        assert_eq!(emitted, first, "serialize #{attempt} produced a different key order");
    }
}

/// The absent key must stay absent: `tty` is `Option<bool>`, and emitting
/// `tty: false` for every ordinary task would both bloat the re-emitted ottofile
/// and turn "unset" into "explicitly off".
#[test]
fn tty_absent_stays_absent_and_present_is_emitted() {
    let yaml = r#"
tasks:
  login:
    action: aws sso login
    tty: true
  build:
    action: cargo build
"#;
    let config: ConfigSpec = serde_yaml_ng::from_str(yaml).expect("parse failed");
    let emitted = serde_yaml_ng::to_string(&config).expect("serialize failed");
    assert!(emitted.contains("tty: true"), "emitted yaml lost tty: true:\n{emitted}");
    assert!(
        !emitted.contains("tty: false"),
        "an unset tty must not be emitted as false:\n{emitted}"
    );
}

/// A minimal config serializes to exactly what it was written as: no
/// null-valued param keys, no `choices: []`, no implicit `nargs: '1'`, and no
/// 17-line default `otto:` block that the ottofile never wrote.
///
/// Before `skip_serializing_if` landed in `src/cfg/`, one param with only a
/// `help:` emitted `metavar: null`, `default: null`, `choices: []`,
/// `choices-command: null`, and `nargs: '1'`, and every config gained a full
/// default `otto:` block on the way out.
#[test]
fn a_minimal_config_round_trips_without_gaining_keys() {
    let yaml = "tasks:\n  build:\n    params:\n      -v|--verbose:\n        help: be loud\n    bash: echo hi\n";

    let config: ConfigSpec = serde_yaml_ng::from_str(yaml).expect("parse");
    let out = serde_yaml_ng::to_string(&config).expect("serialize");

    assert_eq!(out, yaml, "round-trip must be byte-identical;\ngot:\n{out}");

    for noise in ["null", "choices: []", "nargs:", "otto:"] {
        assert!(
            !out.contains(noise),
            "serialized form must not contain {noise:?};\ngot:\n{out}"
        );
    }
}
