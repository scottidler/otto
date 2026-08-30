# Ottofile Key Reference

Every key an ottofile (`.otto.yml`/`otto.yml`/`otto.yaml`) can contain. Since
`docs/design/2026-08-29-strict-ottofile-schema.md` shipped, any key NOT on
this page is a hard, loud config-load error naming the field, its path, and
(usually) its line/column — not a silent no-op. This page exists because its
prior absence was the documented root cause of two work repos inventing a
top-level `envs:` key that never did anything (see the migration note,
`docs/commands/ottofile-strict-schema-migration.md`).

Levels below mirror the six Rust structs `deny_unknown_fields` is applied to
(design doc Architecture table), plus one more: `EdgeSpec` (`src/cfg/edge.rs`),
which is not in that table but enforces the identical "unknown field" contract
by hand via a hand-written `visit_map` rather than the derive macro — see
**Edge object** below. "Type" is the on-disk YAML shape, not the Rust type.
"Default" is what applies when the key is omitted; "required" means omitting
it is fine only in the sense that nothing defaults it to a useful value (there
is no ottofile-level "required" key at all — every field across all seven
structs has a default, except `EdgeSpec.task`, which is genuinely required).

## Root (`ConfigSpec`) — 2 keys

| key | type | default | notes |
|---|---|---|---|
| `otto` | map | `{}` (all `otto.*` defaults) | See **`otto:`** below. |
| `tasks` | map: task name -> task | `{}` | See **`tasks.<name>:`** below. Free-form key site: the map KEYS are task names, not a fixed field list. |

**No other root key exists.** In particular there is no root `envs:` — global
environment variables live at `otto.envs`, one level down. See the migration
note for the two work repos this broke.

## `otto:` (`OttoSpec`) — 7 keys

**Five more keys used to parse here and do nothing**: `home`, `verbosity`,
and (before 2026-08-30) `jobs` were accepted-and-ignored — `deny_unknown_fields`
made them load without error, which made a dead key look official. `home` and
`verbosity` had no reader anywhere in `src/` and no committed ottofile ever
set them; they were deleted from the schema rather than wired, since otto's
state directory already has one real knob (`$OTTO_HOME`, see
`src/executor/layout.rs`) and inventing a second, competing one would only
create ambiguity about which wins. `name` and `about` also had no reader, but
`otto Convert` writes `about` into every Makefile it converts and a dozen
example ottofiles set both, so they were wired into `otto --help`'s title and
description instead of deleted. `jobs` was wired the same way: a dozen
example ottofiles set it as if it worked, so it now IS the default
concurrency, applied only when `-j/--jobs` was not given explicitly on the
command line (the flag still always wins).

| key | type | default | notes |
|---|---|---|---|
| `otto.name` | string | `"otto"` | The `Command` name `otto --help` renders under, when an ottofile parsed successfully. Has no effect on the initial arg-parse or on the "no ottofile" fallback help, which stay `"otto"` since neither has a `ConfigSpec` to read yet. |
| `otto.about` | string | `"A task runner"` | The one-line description shown by the same `otto --help` path as `name`. `otto Convert` sets this to `"Converted from Makefile"` in its output. |
| `otto.api` | string | `"1"` | **Schema version gate**, checked before the strict parse. Must be one of the versions this otto's `SUPPORTED_API_VERSIONS` const supports (currently just `"1"`); an unsupported value is rejected with a message naming the declared version, the supported set, and "upgrade otto" — before any unknown-key error, so a newer ottofile on an older otto gets a truthful message instead of a confusing one about whichever key is new. |
| `otto.jobs` | integer | number of CPUs | Default concurrent-task limit, used only when `-j/--jobs` is not passed on the command line. |
| `otto.tasks` | list of strings | `["*"]` | Default-task-selection filter (which tasks run when none are named on the command line). **Not** the task map — that is root `tasks:`, a different key at a different level. Naming collision between the two is real; do not confuse them. |
| `otto.envs` | map: string -> string | `{}` | **Global environment variables**, available to every task. Free-form key site: the map keys are env-var names, not a fixed field list. This is the key two work repos invented at the wrong level (root `envs:`) before this page existed — see the migration note. |
| `otto.retention` | map | `RetentionSpec` defaults | See **`otto.retention:`** below. |

## `otto.retention:` (`RetentionSpec`) — 5 keys

**Every field here is plain snake_case, with no kebab rename** — unlike
`tasks.<name>.on-failure` and `tasks.<name>.params.<title>.choices-command`,
both kebab. Writing `keep-days` (kebab) here is a **hard error** today: it is
not accepted, not aliased, and not silently defaulted. That inconsistency
between this block and the two kebab keys elsewhere in the schema is real and
unresolved; unifying the convention (with aliases and a deprecation window)
is its own future doc, not this one.

| key | type | default | notes |
|---|---|---|---|
| `otto.retention.keep_days` | integer | `30` | Delete runs older than this many days. |
| `otto.retention.keep_last` | integer | `10` | Always keep at least this many most recent runs. |
| `otto.retention.keep_failed` | integer | `60` | Keep failed runs for this many days. |
| `otto.retention.auto_prune` | boolean | `true` | Enable automatic pruning after runs. |
| `otto.retention.prune_interval_hours` | integer | `24` | Minimum hours between auto-prune runs. |

`otto Convert`'s emitted `retention:` block uses exactly these snake_case
names, so its own output stays loadable.

## `tasks.<name>:` (`TaskSpecHelper`) — 13 keys

The root `tasks:` map's keys are task names (free-form; see Root above). Each
value is a task, whose own keys are fixed and listed here.

| key | type | default | notes |
|---|---|---|---|
| `tasks.<name>.help` | string | none | Help text shown for this task. |
| `tasks.<name>.after` | list of edges | `[]` | Tasks this one runs after. Each edge is either a bare task-name string (sugar, implies `when: success`) or a map `{task: <name>, when: success\|failure\|always}`. See **Edge object** below. |
| `tasks.<name>.before` | list of edges | `[]` | Tasks this one runs before. Same edge shape as `after`. See **Edge object** below. |
| `tasks.<name>.input` | list of strings | `[]` | File glob(s) this task reads; drives up-to-date skipping. |
| `tasks.<name>.output` | list of strings | `[]` | File glob(s) this task writes; drives up-to-date skipping. |
| `tasks.<name>.envs` | map: string -> string | `{}` | Task-scoped environment variables, layered over `otto.envs`. Free-form key site, same as `otto.envs`. |
| `tasks.<name>.params` | map: param title -> param | `{}` | See **`tasks.<name>.params.<title>:`** below. Free-form key site: the map keys are param titles (e.g. `-v\|--verbose`), parsed by `divine()`, not a fixed field list. |
| `tasks.<name>.bash` | string | none | Bash script body to run. |
| `tasks.<name>.python` | string | none | Python script body to run. |
| `tasks.<name>.action` | string | none | Legacy path to an executable action. Deprecated in favor of `bash`/`python`. |
| `tasks.<name>.foreach` | map | none | Dynamic subtask generation. See **`tasks.<name>.foreach:`** below. |
| `tasks.<name>.on-failure` | list of strings | `[]` | **Kebab key.** Task names to run when this task fails; parse-time sugar that desugars into `after:` edges with `when: failure` on the named tasks. |
| `tasks.<name>.tty` | boolean | none (absent = false) | Give this task the terminal: inherit stdout/stderr instead of capturing them, drop the `[task]` output prefix, and run it exclusively (no other task runs alongside it). |

**`parallel:` is not a task-level key.** It belongs under `foreach:` — see
below. Writing it here (the bug this whole design doc was written to catch)
is now a loud `unknown field 'parallel'` error naming path `tasks.<name>`.

## Edge object (`EdgeSpec`) — 2 keys

Each entry in `tasks.<name>.after`/`tasks.<name>.before` is either a bare
task-name string (sugar for `{task: <name>, when: success}`, and the form
`otto` always writes back for a `success`-only edge) or a map with these two
keys. `EdgeSpec` is not one of the six Architecture-table structs — it isn't
`#[serde(deny_unknown_fields)]` — but its hand-written `visit_map`
(`src/cfg/edge.rs`) rejects any key besides these two with the same
"unknown field" error shape the derive macro produces, so it is inventoried
here too.

| key | type | default | notes |
|---|---|---|---|
| `tasks.<name>.after[].task` | string | none (**required**) | Name of the task this edge refers to. Same key at `tasks.<name>.before[].task`. |
| `tasks.<name>.after[].when` | string: `success`\|`failure`\|`always` | `success` | Condition under which this edge fires. Same key at `tasks.<name>.before[].when`. |

## `tasks.<name>.foreach:` (`ForeachSpec`) — 7 keys

| key | type | default | notes |
|---|---|---|---|
| `tasks.<name>.foreach.glob` | string | none | File glob pattern; each match becomes one subtask item. |
| `tasks.<name>.foreach.items` | list of strings | `[]` | Explicit list of items. |
| `tasks.<name>.foreach.range` | string | none | Numeric range, e.g. `"1-10"` (1 through 10 inclusive). |
| `tasks.<name>.foreach.command` | string | none | Shell command whose stdout lines become the items. Resolves lazily (never for `--help`) and at most once per invocation. Mutually exclusive with `glob`/`items`/`range`. |
| `tasks.<name>.foreach.as` | string | `"item"` | **Kebab-shaped on disk** (the Rust field is `var_name`, renamed). Variable name bound to the current item in each subtask. |
| `tasks.<name>.foreach.parallel` | boolean | `true` | Whether subtasks run concurrently or serially. This is the key that must live HERE, not one level up on the task — the motivating bug for this whole design doc was `parallel:` written beside `foreach:` instead of inside it. |
| `tasks.<name>.foreach.max_items` | integer | `1000` | Maximum item count before erroring. |

## `tasks.<name>.params.<title>:` (`ParamSpec`) — 6 keys

The `params:` map's keys are **rich titles**, e.g. `-v|--verbose`, `-s`,
`--service`, or a bare word for a positional param — free-form, parsed by
`divine()` into the derived (non-writable) `name`/`short`/`long` fields.
`divine()`'s own tolerance for malformed titles (e.g. `-verbose` misread as
positional) is a separate, still-open hazard tracked in
`docs/design/2026-06-10-code-review-remediation.md:217`, not this page.

Each param VALUE's own keys are fixed and listed here. **Five fields are
`#[serde(skip)]`** (`name`, `short`, `long`, `param_type`, `value`): they are
derived from the title by `divine()` or populated at runtime, never
user-writable, and therefore not listed as ottofile keys at all — writing any
of them under a param (e.g. `name: foo`) is now a rejected, not merely
ignored, key.

| key | type | default | notes |
|---|---|---|---|
| `...params.<title>.metavar` | string | none | Placeholder name shown in help/usage output. |
| `...params.<title>.default` | string | none | Default value when the param is unset. |
| `...params.<title>.choices` | list of strings | `[]` | Static allowed-value set. |
| `...params.<title>.choices-command` | string | none | **Kebab key.** Shell command whose stdout lines become the allowed value set (dynamic choices), resolved lazily and at most once per invocation. |
| `...params.<title>.nargs` | string | `"1"` | One of `"0"`, `"1"`, `"?"` (zero-or-one), `"+"` (one-or-more), `"*"` (zero-or-more), a bare integer `"N"` (max count, min 0), or `"N:M"` (min:max, 1-indexed on disk). Wired to clap's `num_args`: a value of more than one collects every space-separated value from one occurrence. |
| `...params.<title>.help` | string | none | Help text shown for this param. |

`dest` and `constant` were removed (design doc `2026-06-10-code-review-remediation.md`,
Phase 6): both parsed and serialized but had zero readers outside
`cfg/param.rs` itself.

## Free-form key sites (do NOT expect `deny_unknown_fields` here)

`deny_unknown_fields` governs a struct's own declared field names; it never
reaches the contents of a map-typed field. Exactly three sites in the schema
accept arbitrary keys:

1. **`tasks:` (root)** — keys are task names, values are `tasks.<name>:`.
2. **`envs:` (both `otto.envs` and `tasks.<name>.envs`)** — keys are
   environment variable names, values are strings.
3. **`params:` (`tasks.<name>.params`)** — keys are rich param titles parsed
   by `divine()`, values are `tasks.<name>.params.<title>:`.

## Total: 44 fixed keys across the seven structs

`ConfigSpec` 2 + `OttoSpec` 9 + `RetentionSpec` 5 + `ForeachSpec` 7 +
`TaskSpecHelper` 13 + `ParamSpec` 6 + `EdgeSpec` 2 = **44**. This count, and
every key name above, is pinned by an automated drift test
(`ottofile_reference_key_inventory_is_exhaustive`, in
`src/cfg/task.rs`'s `#[cfg(test)]` module): it destructures a live instance of
each struct (a compile-time trigger — the build breaks the moment a struct
gains or loses a field, before any test even runs) and separately recovers
each struct's real on-disk key list from its "unknown field" error message
(fed a deliberately bogus key — `deny_unknown_fields`'s derive-generated
error for the six Architecture-table structs, `EdgeSpec`'s hand-written
`visit_map` error for the seventh), then asserts every one of those 44
recovered keys is mentioned, verbatim, on this page. If this page and the
schema ever drift, `cargo test` fails — it does not merely go stale quietly.
