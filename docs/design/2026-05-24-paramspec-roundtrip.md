# ParamSpec round-trip asymmetry — design question

**Status:** Open question, not yet implemented
**Date:** 2026-05-24

## Context

Otto's `ConfigSpec` (defined in `src/cfg/`) describes a `.otto.yml` file: a top-level `otto:` block, a `tasks:` map, and each `TaskSpec` carries a `params:` map of CLI parameters (`ParamSpec`).

The parameter map in YAML uses *human-readable keys* that encode the CLI flag form:

```yaml
tasks:
  test:
    params:
      "-v|--verbose":        # <-- the map key encodes short + long
        default: false
        help: Enable verbose output
      "-e|--env":
        default: development
        choices: [development, staging, production]
      "filename":             # <-- positional, no flag prefix
        help: Input filename
```

On deserialize, `deserialize_param_map` in `src/cfg/param.rs:200` walks the map and calls `divine(title)` on each key to compute `name`, `short`, and `long` from the key form. Those three become struct fields on `ParamSpec`:

```rust
pub struct ParamSpec {
    #[serde(skip_deserializing)]
    pub name: String,              // derived from key

    #[serde(skip_deserializing)]
    pub short: Option<char>,       // derived from key

    #[serde(skip_deserializing)]
    pub long: Option<String>,      // derived from key

    #[serde(skip_deserializing, default)]
    pub param_type: ParamType,     // derived from key + default value
    // ... other fields that ARE deserialized
}
```

`param_type` is then computed by `deserialize_param_map`: if `short` or `long` are set and `default` is `"true"`/`"false"`, it becomes `FLG`; otherwise `OPT`; otherwise (no short/long) `POS`.

After all of that, the `params` HashMap is keyed by `name` (the result of `divine`), not by the original rich title — that information is consumed and discarded.

## The asymmetry

Because the map key during deserialize carries information that's spread across four `ParamSpec` fields, the round-trip via `serde_yaml::to_string(&config_spec)` is lossy by construction:

- `name`/`short`/`long`/`param_type` ARE serialized to the YAML output (`skip_deserializing` only skips the input side).
- But the params map key on serialize is just `name`, e.g. `"verbose"`, not `"-v|--verbose"`.
- On re-parse, `divine("verbose")` returns `(name="verbose", short=None, long=None)`, then `deserialize_param_map` sets `param_type = POS` because there's no short/long. The `name`/`short`/`long`/`param_type` fields the previous serialize emitted as YAML are ignored on re-deserialize (because of `skip_deserializing`).

Net effect on real ottofiles: parse 94 files, re-emit, re-parse → 74/94 fail to structurally equal the original. Every drift case is the same shape — a `FLG`/`OPT` param decays to `POS` with `short=None`, `long=None`.

## Why this matters now

I just fixed two adjacent (smaller) round-trip bugs in v1.2.1 and v1.2.2 — `Nargs` and `Value` enums whose derived `Serialize` impls emitted variant names that their hand-written `Deserialize` impls didn't accept. Those fixes are real, but they didn't move the 74/94 drift number: every drift is dominated by this `ParamSpec` key asymmetry.

## Current blast radius

The only production code path in this repo that re-serializes a `ConfigSpec` is `src/cli/commands/convert.rs:39` — `otto convert <Makefile`. The Makefile converter (`src/makefile/converter.rs:100`) always emits `params: HashMap::new()`, so today no user-visible flow triggers the bug.

But it's a latent foot-gun for any future feature that emits param-bearing configs — examples that have been informally floated:
- An `otto format` / `otto pretty` command that normalizes an existing `.otto.yml`
- A `--dump-config` flag that exposes the resolved config
- A converter for argparse-style Python tools or `--help` text

## Options

### Option A — Custom Serialize for the params map: re-encode the key from `name`/`short`/`long`

Implement a custom serializer for the `params` map (or a `SerializeWith` for `TaskSpec.params`) that reconstructs the original-style key from the `ParamSpec`'s `name`/`short`/`long`/`param_type` fields:

```rust
fn key_for(spec: &ParamSpec) -> String {
    match (spec.short, &spec.long) {
        (Some(s), Some(l)) => format!("-{s}|--{l}"),
        (Some(s), None)    => format!("-{s}"),
        (None,    Some(l)) => format!("--{l}"),
        (None,    None)    => spec.name.clone(),  // positional
    }
}
```

Pros:
- Lossless round-trip — every existing `.otto.yml` re-serializes to a form that re-deserializes equal.
- Output looks like the source — humans reading dumped configs see the form they expect.

Cons:
- Requires the four computed fields to also be skipped on serialize (otherwise they appear twice: once in the key, once inline).
- Two sources of truth temporarily (the rich key AND the inline fields). If the in-memory `ParamSpec` is mutated such that `name` no longer matches `divine(key)`, the next round-trip silently drifts. Today there's no code that mutates these fields post-parse, but the contract is implicit.
- Introduces a custom Serializer for `HashMap<String, ParamSpec>` — modest complexity.

### Option B — Mirror `skip_deserializing` with `skip_serializing` on the four computed fields

Stop emitting `name`/`short`/`long`/`param_type` in the YAML output entirely. Output becomes:

```yaml
params:
  verbose:        # name only
    default: false
    help: ...
```

Re-parse loses flag info (decays to `POS`) — same as today's drift behavior — but the *emitted YAML doesn't lie* about what was there.

Pros:
- Trivial change (4 attribute additions).
- Makes the asymmetry explicit at the type level instead of hiding behind serde defaults.
- No new sources-of-truth.

Cons:
- Doesn't actually fix the round-trip — still 74/94 drift, just with smaller per-file diffs.
- Future consumers that emit param-bearing configs will still produce broken `.otto.yml` files. Closes nothing.

### Option C — Defer

Document the asymmetry as a known issue. Don't change anything. Revisit when a real consumer appears.

Pros:
- Zero code risk today.
- Forces the design conversation to happen when there's concrete usage to design against.

Cons:
- Adds an unmarked landmine. Any contributor adding a feature that serializes a param-bearing config will produce wrong output, and the failure mode is silent (passes type check, parses, just loses information).

### Option D — Reshape the type so the asymmetry can't exist

The asymmetry exists because four fields of a struct are derived from the *position* of that struct inside a map (the map key). If `name`/`short`/`long`/`param_type` weren't part of `ParamSpec` at all, the problem wouldn't exist — the key would be the only source of truth.

This would mean: parse the params map into a `Vec<(RichKey, ParamSpec)>` (or a custom map type whose key carries the rich form), and derive `name`/`short`/`long`/`param_type` at the consumption sites — when building clap commands, when matching CLI args.

Pros:
- Removes the duplication entirely. Single source of truth.
- Aligns with the actual data model: the rich key IS the identifier.

Cons:
- Widest-blast refactor. Touches every site that reads `param.name`/`param.short`/`param.long`/`param.param_type` — likely dozens of call sites in `executor/`, `app/`, possibly `cli/`.
- Hard to ship as one change without coordination with other in-flight work.

## The question for the Architect

Given the current blast radius (latent, no live consumer), which option matches Otto's design intent? Specifically:

1. Is the key-as-identifier pattern (`"-v|--verbose"`) a deliberate UX choice that should be preserved in any serialization output (favors A or D), or is it a parsing convenience that should be regularized away in canonical form (favors B with `name`-only keys)?

2. Option A introduces an implicit invariant that `name`/`short`/`long` stay coherent with what `divine()` would produce from the key. Is that invariant safe to rely on given the codebase's other patterns? Are there other places in `src/cfg/` where the same shape of "field derived from container position" exists, and how are they handled?

3. Are there modes I haven't considered — e.g., should `ParamSpec` carry a `rich_key: String` field that's read on deserialize and written on serialize, eliminating the need for either custom Serializer logic or `divine()` reconstruction? That would split the difference between A and D.

4. Independent of option, is the broader symptom — that `Serialize` impls on `Nargs`/`Value`/`ParamSpec` were never tested against the project's actual `Deserialize` paths until I added the round-trip tests in v1.2.1/v1.2.2 — a sign of a missing test discipline that should be addressed structurally (e.g., a property-test invariant that every `Deserialize`-able config type must round-trip), rather than per-fix?

## Files to verify against

- `src/cfg/param.rs` — `ParamSpec`, `divine`, `deserialize_param_map`, `Nargs`, `Value`
- `src/cfg/task.rs` — `TaskSpec` (holder of params map)
- `src/cfg/config.rs` — `ConfigSpec`
- `src/cli/commands/convert.rs:39` — the one production serialize site
- `src/makefile/converter.rs:100` — proof that the live serialize site doesn't currently emit params
- `app/` and `executor/` — consumers that read `param.name`/`param.short`/`param.long`/`param.param_type` (for Option D blast-radius)
