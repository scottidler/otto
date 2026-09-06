# Ottofile Key Reference

Every key an ottofile (`otto.yml`, `.otto.yml`, `otto.yaml`, `.otto.yaml`,
`Ottofile`, or `OTTOFILE`) can contain. Since
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

## `otto:` (`OttoSpec`) — 8 keys

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
| `otto.envs-command` | string | none | **Kebab key.** Shell command whose `KEY=VALUE` stdout becomes global environment variables, layered UNDER `otto.envs` (a literal `otto.envs` entry for the same key still wins). Runs with the ottofile's directory as cwd, at most once per invocation, lazily: never for `--help`, otherwise whenever something needs the env map. Values are taken literally - no unquoting, no `$(...)` re-evaluation, no `${VAR}` expansion - so a value cannot contain a newline; multi-line values stay in `otto.envs`. Blank lines and `#` comment lines are skipped; a line with no `=` or an invalid key is a load error naming the line number. Empty output is legal and means "no variables". |
| `otto.retention` | map | `RetentionSpec` defaults | See **`otto.retention:`** below. |

## `otto.retention:` (`RetentionSpec`) — 5 keys

**Every field here is plain snake_case, with no kebab rename** — unlike
`otto.envs-command`, `tasks.<name>.on-failure`, and
`tasks.<name>.params.<title>.choices-command`, all three kebab. Writing
`keep-days` (kebab) here is a **hard error** today: it is not accepted, not
aliased, and not silently defaulted. That inconsistency between this block and
the three kebab keys elsewhere in the schema is real and unresolved; unifying
the convention (with aliases and a deprecation window) is its own future doc,
not this one.

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
| `tasks.<name>.after` | list of edges | `[]` | Tasks that run after this one (this task becomes their dependency). Each edge is either a bare task-name string (sugar, implies `when: success`) or a map `{task: <name>, when: success\|failure\|always}`. See **Edge object** below. |
| `tasks.<name>.before` | list of edges | `[]` | Tasks that run before this one (they become this task's dependencies). Same edge shape as `after`. See **Edge object** below. |
| `tasks.<name>.input` | list of strings | `[]` | File glob(s) this task reads; drives up-to-date skipping. |
| `tasks.<name>.output` | list of strings | `[]` | File glob(s) this task writes; drives up-to-date skipping. |
| `tasks.<name>.envs` | map: string -> string | `{}` | Task-scoped environment variables, layered over `otto.envs`. Free-form key site, same as `otto.envs`. |
| `tasks.<name>.params` | map: param title -> param | `{}` | See **`tasks.<name>.params.<title>:`** below. Free-form key site: the map keys are param titles (e.g. `-v\|--verbose`), parsed by `divine()`, not a fixed field list. |
| `tasks.<name>.bash` | string | none | Bash script body to run. |
| `tasks.<name>.python` | string | none | Python script body to run. |
| `tasks.<name>.action` | string | none | Legacy path to an executable action. Deprecated in favor of `bash`/`python`. |
| `tasks.<name>.foreach` | map | none | Dynamic subtask generation. See **`tasks.<name>.foreach:`** below. |
| `tasks.<name>.on-failure` | list of strings | `[]` | **Kebab key.** Task names to run when this task fails; parse-time sugar that desugars into `after:` edges with `when: failure`, pushed onto this task's own `after:` list and pointing at the named tasks. |
| `tasks.<name>.tty` | boolean | none (absent = false) | Give this task the terminal: inherit stdout/stderr instead of capturing them, drop the `[task]` output prefix, and run it exclusively (no other task runs alongside it). A non-`tty` task cannot read the terminal: stdin is `/dev/null` when otto's is a terminal, and it has no controlling terminal, so `/dev/tty` cannot be opened. Set `tty: true` for anything that prompts. |

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

### Worked example: which task runs first

```yaml
tasks:
  first:
    help: "the task invoked directly"
    bash: "sleep 1 && echo first-done"
  later:
    help: "declares after: [first]"
    after: [first]
    bash: "echo later"
```

Invoked as `otto first`. `later`'s `after: [first]` makes `later` a
dependency of `first` (its own entry says so: "Tasks that run after this
one, this task becomes their dependency"), so `later` runs first, then
`first` starts. Measured with wall-clock timestamps:

```
later      1788676252.533636807
first      1788676252.578740988
first-done 1788676253.590615515
```

`later` ran 45ms before `first`, while `first` was still sleeping. To make
`later` run after `first` finishes, either declare `after:` on `first`
pointing at `later` (`first: {after: [later]}`), or declare `before:` on
`later` pointing at `first` (`later: {before: [first]}`); both make
`later` depend on `first`.

## `tasks.<name>.foreach:` (`ForeachSpec`) — 9 keys

Exactly one of `command`, `glob`, `items`, `range` must be set: zero sources
and two sources are both rejected at load, naming the task and every source
found.

| key | type | default | notes |
|---|---|---|---|
| `tasks.<name>.foreach.glob` | string | none | File glob pattern; each match becomes one subtask item. |
| `tasks.<name>.foreach.items` | list of strings | `[]` | Explicit list of items. |
| `tasks.<name>.foreach.range` | string | none | Numeric range, e.g. `"1-10"` (1 through 10 inclusive). Counted at load, not expanded: a range wider than `max_items` is a config error before a single item exists. |
| `tasks.<name>.foreach.command` | string | none | Shell command whose stdout lines become the items. Resolves lazily (never for `--help`) and at most once per invocation. Mutually exclusive with `glob`/`items`/`range`. |
| `tasks.<name>.foreach.as` | string | `"item"` | **Kebab-shaped on disk** (the Rust field is `var_name`, renamed). Variable name bound to the current item in each subtask. It becomes a shell variable, so it must be an identifier (letters, digits and underscore, not starting with a digit); anything else is rejected at load naming `foreach.as`. |
| `tasks.<name>.foreach.parallel` | boolean | `true` | Whether subtasks run concurrently or serially. This is the key that must live HERE, not one level up on the task — the motivating bug for this whole design doc was `parallel:` written beside `foreach:` instead of inside it. |
| `tasks.<name>.foreach.max_items` | integer | `1000` | Maximum item count before erroring. |
| `tasks.<name>.foreach.buffer` | boolean | `false` | Run subtasks concurrently but print each subtask's output as one contiguous block, in item order. Rejected at load if the task also sets `tty: true` (a tty task owns the terminal exclusively). |
| `tasks.<name>.foreach.jobs` | `"all"` or positive integer | none | Concurrency for THIS group's items, overriding the global `-j`/`otto.jobs`: `all` gives one permit per item (for a group of tasks that never exit on their own, e.g. a log tail), or a fixed integer caps the group at that count. Rejected at load together with `parallel: false` (incoherent: serial already means one at a time), and `jobs: 0` is rejected in favor of writing `all`. |

## `tasks.<name>.params.<title>:` (`ParamSpec`) — 7 keys

The `params:` map's keys are **rich titles**, e.g. `-v|--verbose`, `-s`,
`--service`, or a bare word for a positional param — free-form, parsed by
`divine()` into the derived (non-writable) `name`/`short`/`long` fields.
`divine()` is fallible: a malformed title is a load error, not a silent
misreading. `-verbose` (a multi-character name behind one dash), `-v|-x` (two
shorts), `--foo|--bar` (two longs), a bare name mixed with a flag, and two
titles that divine to the same name are each rejected by name (`divine()` in
`src/cfg/param.rs`, pinned by `divine_rejects_*` and
`deny_duplicate_divined_names_in_one_params_map` in `src/cfg/param_tests.rs`).

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
| `...params.<title>.nargs` | string | `"1"` | One of `"0"`, `"1"`, `"?"` (zero-or-one), `"+"` (one-or-more), `"*"` (zero-or-more), a bare integer `"N"` (exactly N), or `"N:M"` (min:max). Wired to clap's `num_args`: a value of more than one collects every space-separated value from one occurrence. A bounded zero-to-N is not expressible: `"0:N"` is rejected at load (`min must be at least 1`), so use `"?"` for zero-or-one or `"*"` for zero-or-more. |
| `...params.<title>.help` | string | none | Help text shown for this param. |
| `...params.<title>.required` | boolean | `false` | clap enforces the value (a usage error instead of an empty variable). Rejected at load together with a `FLG` param, `default:`, or an `nargs` of `"0"`/`"?"`/`"*"` (all mean "may appear zero times"), and rejected if it declares a required positional after an optional one (clap panics on that shape). Fires only when the task is named on the command line: a task pulled in only as a dependency has no CLI partition and runs with the param unset, matching `choices`. |

`dest` and `constant` were removed (design doc `2026-06-10-code-review-remediation.md`,
Phase 6): both parsed and serialized but had zero readers outside
`cfg/param.rs` itself.

## Passing data between tasks: `otto_set_output` and `otto_get_input`

Otto defines two shell functions inside every `bash:`/`action:` body.

```bash
otto_set_output "<key>" "<value>"   # in the producing task
otto_get_input  "<task>.<key>"      # in a task that declares `before: [<task>]`
```

A consumer only sees a producer's outputs if it declares the dependency; there
is no ambient sharing.

```yaml
otto:
  api: 1
tasks:
  generate:
    bash: otto_set_output "stamp" "$(date +%s)"
  consume:
    before: ["generate"]
    bash: echo "got $(otto_get_input "generate.stamp")"
```

### What happens when the key is not there

`otto_get_input` prints a diagnostic naming the key and what *was* available, and
returns non-zero:

```
otto: no input 'generate.missing'; available: generate.stamp
```

**Read the message, not the exit status.** The message is emitted in every case;
the non-zero return only reaches your script in one of the three common shapes,
because bash discards the status of a command substitution in the other two:

| Shape | Exit status | Result |
|---|---|---|
| `x=$(otto_get_input k)` under `set -e` | task fails, rc 1 | the assignment *is* the command, so `set -e` sees the failure |
| `local x=$(otto_get_input k)` | rc 0 | `local` succeeds; its status replaces the substitution's |
| `echo "[$(otto_get_input k)]"` | rc 0 | `echo` succeeds; the empty value is interpolated |

If you want a default rather than a failure, ask for one explicitly:

```bash
value=$(otto_get_input "generate.stamp") || value="fallback"
```

Keys keep the spelling the producer wrote: `otto_get_input "gen.MixedCase"` needs
that exact case. Task names containing `.` or `-` work, and two tasks whose names
differ only by a separator (`build` and `build_all`) do not collide.

## Variable interpolation in `envs`, `input`, and `output`

Otto expands variable references in these values *before* the shell ever sees
them. Three forms are recognised, in one left-to-right pass, and substituted
text is never rescanned:

| Form | Meaning |
|---|---|
| `${NAME}` | The value of `NAME`, from `tasks.<name>.envs` layered over `otto.envs`. Braces are required when the next character could continue the name. |
| `$NAME` | The same, where `NAME` runs to the first character that is not a letter, digit, or `_`. |
| `$$` | **A literal `$`.** Consumed by otto, emitted as one dollar sign. |

An unresolved `${NAME}` is an error naming the variable, not an empty string:

```
Environment variable 'NAME' not found
```

### `$$`: putting a literal dollar sign in a value

`$$` is the only way to get a `$` through to the shell without otto trying to
resolve it first. Without it, there is no way to write a value containing a
dollar sign that bash will not then read as a variable or as its own PID.

```yaml
otto:
  api: 1
  envs:
    PRICE: "$$4.99"          # the task sees the five characters $4.99
    AWK_FIRST_FIELD: "$$1"   # the task sees $1, not the value of $1
tasks:
  show:
    action: |
      echo "$PRICE"
```

**`action:` bodies are not interpolated by otto.** They are handed to the shell
verbatim, so a `$` there means whatever bash says it means, and `$$` inside an
action is bash's own PID. Write `awk '{print $1}'`, not `awk '{print $$1}'`, in
an action: the doubled form reaches awk as `$$1` and prints the wrong field.
`$$` is for `envs`, `input`, and `output` values only.

Two more consequences worth knowing:

- **`$$` wins over the other forms.** `$${VAR}` is a literal `$` followed by
  the four characters `{VAR}`; it does *not* expand `VAR`. Likewise `$$(echo hi)`
  is a literal `$` followed by `(echo hi)` as text, not a command substitution.
- **An unterminated `${` is literal text.** `"${"` passes through as `${`
  rather than erroring.

### Command substitution, and `${VAR:-default}`

Otto does not implement shell parameter expansion. `${MYVAR:-fallback}` is read
as a *variable named* `MYVAR:-fallback` and fails:

```
Failed to evaluate global environment variables: Failed to resolve environment
variable 'G': Environment variable 'MYVAR:-fallback' not found
```

To use a shell default, defer it to the shell with a command substitution,
which otto passes through:

```yaml
    envs:
      GREETING: "$(echo \"${MYVAR:-fallback}\")"
```

## Free-form key sites (do NOT expect `deny_unknown_fields` here)

`deny_unknown_fields` governs a struct's own declared field names; it never
reaches the contents of a map-typed field. Exactly three sites in the schema
accept arbitrary keys:

1. **`tasks:` (root)** — keys are task names, values are `tasks.<name>:`.
2. **`envs:` (both `otto.envs` and `tasks.<name>.envs`)** — keys are
   environment variable names, values are strings.
3. **`params:` (`tasks.<name>.params`)** — keys are rich param titles parsed
   by `divine()`, values are `tasks.<name>.params.<title>:`.

## Environment and shell helpers

None of what follows is declared in the ottofile. otto injects these
environment variables and shell functions into a task's execution
environment itself, or reads them from its own environment at startup.

### Per-task variables

Set on every task's `Command` before spawn
(`src/executor/scheduler/task_execution.rs`, `execute_task`):

| Variable | Value |
|---|---|
| `OTTO_TASK` | The task's name. |
| `OTTO_TASK_DIR` | This task's run directory: `.../tasks/<task-name>/` (see [`docs/directory-layout.md`](../directory-layout.md)). |
| `OTTO_WORKSPACE` | The current project directory (`<name>-<hash>/`), the parent of every run. |
| `OTTO_TASKS_DIR` | The current run's `tasks/` directory, the parent of every task's `OTTO_TASK_DIR`. |
| `OTTO_USER` | The user otto is running as (`$USER`, or `unknown` if unset). |

A foreach subtask additionally gets (`src/cfg/task.rs`, `expand_foreach_with_items`):

| Variable | Value |
|---|---|
| `OTTO_FOREACH_ITEM` | The item's value. Also bound under the name `foreach.as` gives it. |
| `OTTO_FOREACH_INDEX` | Its zero-based position in declaration order. |

One `OTTO_INPUT_<TASK>_<KEY>` variable also lands per key a declared
dependency produced with `otto_set_output`, folded through the rule
described just above in **Passing data between tasks**.

### Variables otto reads from its own environment

Read once, at startup or first use, from whatever environment ran `otto` -
these are not injected into a task, though a task inherits any of them that
were already set in that shell:

| Variable | Meaning |
|---|---|
| `OTTO_HOME` | Overrides otto's state directory (default `$HOME/.otto`; `src/executor/layout.rs`). |
| `OTTO_DB_PATH` | Overrides the SQLite database path, independent of `OTTO_HOME` (`src/executor/state/db.rs`). |
| `OTTO_MAX_LOG_BYTES` | Overrides the 10 MB threshold at which `main.rs` rotates `otto.log` to `otto.log.1`. |
| `OTTOFILE` | An alternate way to set `-o`/`--ottofile`; the flag wins if both are given. |

### Bash color variables

Every `bash:`/`action:` body sourcing the generated `builtins.sh` gets nine
ANSI color constants for free (`src/executor/action.rs`, `create_builtins`):
`RED`, `GREEN`, `YELLOW`, `BLUE`, `MAGENTA`, `CYAN`, `WHITE`, `BOLD`, `DIM`,
and `NC` (reset).

### Shell functions

`otto_set_output`/`otto_get_input` are documented above, for cross-task data
passing. Two more functions exist alongside them, but a task body does not
call them directly - otto's generated prologue and epilogue call them
automatically (`src/executor/action.rs:416-429,526`):

| Function | Called from | Does |
|---|---|---|
| `otto_serialize_output` | The generated epilogue, once per task. | Writes `OTTO_OUTPUT` to `output.<task>.json`/`.env`. |
| `otto_deserialize_input` | The generated prologue, once per declared dependency. | Reads that dependency's `input.<dep>.env` into `OTTO_INPUT`. |

The same four names exist as Python functions inside the generated
`otto_builtins.py`, bound at module level, for tasks whose interpreter is
Python rather than bash (`src/executor/action.rs:783-786`).

### Per-task option: `--Serial`

A task declaring `foreach:` gets one extra CLI flag that is per-task rather
than global, so it does not appear in `otto --help`'s option table (see
[`docs/grammar.md`](../grammar.md) for the full flag grammar) -
(`src/cli/builtins.rs`, `BUILTIN_PARAMS`; injected in
`src/cli/parser/command.rs`, `task_to_command`):

| Flag | Effect |
|---|---|
| `--Serial` | Run this task's foreach subtasks one at a time instead of in parallel - the command-line equivalent of `foreach.parallel: false`. Rejected at load together with `foreach.jobs` on the same task: an ordering constraint and a concurrency cap are the same incoherence. |

## Total: 46 fixed keys across the seven structs

`ConfigSpec` 2 + `OttoSpec` 8 + `RetentionSpec` 5 + `ForeachSpec` 9 +
`TaskSpecHelper` 13 + `ParamSpec` 7 + `EdgeSpec` 2 = **46**. This count, and
every key name above, is pinned by an automated drift test
(`ottofile_reference_key_inventory_is_exhaustive`, in
`src/cfg/task.rs`'s `#[cfg(test)]` module): it destructures a live instance of
each struct (a compile-time trigger — the build breaks the moment a struct
gains or loses a field, before any test even runs) and separately recovers
each struct's real on-disk key list from its "unknown field" error message
(fed a deliberately bogus key — `deny_unknown_fields`'s derive-generated
error for the six Architecture-table structs, `EdgeSpec`'s hand-written
`visit_map` error for the seventh), then asserts every one of those 46
recovered keys is mentioned, verbatim, on this page. The stated total and the
per-struct arithmetic in this section are pinned by the same test, so a wrong
count here is a red build rather than a footnote nobody re-adds up. If this
page and the schema ever drift, `cargo test` fails — it does not merely go
stale quietly.
