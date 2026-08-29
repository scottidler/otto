//! Round-trip property tests: parse a YAML config, re-emit, re-parse,
//! assert structural equality.
//!
//! This guards against `Serialize` / `Deserialize` asymmetries on config
//! types. Each new variant added to `Nargs`, `Value`, `When`, or any
//! ConfigSpec-reachable enum must round-trip cleanly here.

use otto::cfg::config::ConfigSpec;

fn assert_roundtrips(yaml: &str) {
    let config: ConfigSpec =
        serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("initial parse failed: {e}\nyaml:\n{yaml}"));
    let emitted = serde_yaml::to_string(&config).unwrap_or_else(|e| panic!("serialize failed: {e}"));
    let reparsed: ConfigSpec =
        serde_yaml::from_str(&emitted).unwrap_or_else(|e| panic!("reparse failed: {e}\nemitted:\n{emitted}"));
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
