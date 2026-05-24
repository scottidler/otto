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

/// Known failing case: ParamSpec's `name`/`short`/`long`/`param_type` fields
/// are derived from the params-map KEY during deserialize, but the serializer
/// emits the map keyed by `name` only — losing the `-v|--verbose` form. On
/// re-parse, `divine()` can't recover short/long, so FLG/OPT params decay
/// to POS.
///
/// See `docs/design/2026-05-24-paramspec-roundtrip.md`. This test will pass
/// once the chosen fix (Option E or A) lands.
#[test]
#[ignore = "ParamSpec key asymmetry, see docs/design/2026-05-24-paramspec-roundtrip.md"]
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
