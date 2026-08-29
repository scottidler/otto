# `otto --tasks` - Machine-Readable Task List

`--tasks` prints the ottofile's task list as data (YAML or JSON) and exits without running anything. It exists so another tool (a doctor script, a wrapper, a generator) can ask "what verbs does this ottofile expose" without scraping `otto --help`.

## Usage

```bash
otto --tasks [--format yaml|json]
```

`--tasks` is a global flag, like `-C`/`--cwd` or `--list-subtasks`: it is handled before any task resolution or execution, and it works from any directory an ottofile can be discovered from.

## Output Format

Output follows otto's house rule for machine-readable output: **TTY-detect, one override**.

| Context | Format |
|---|---|
| stdout is a terminal | YAML |
| stdout is piped or redirected | JSON |
| `--format yaml` given | YAML, regardless of tty |
| `--format json` given | JSON, regardless of tty |

There is no boolean `--json` flag. A `jq` consumer pipes with no flag at all (`otto --tasks | jq ...`); a `sed`-only consumer that never pipes into `jq` uses `--format yaml` and reads top-level keys as lines.

## Frozen Contract

This shape is a contract other tools build against. It will not change without a version bump:

- **User-defined ottofile tasks only.** No injected builtins (`Clean`, `Convert`, `Graph`, `History`, `Stats`, `Upgrade`) appear in the map. Consumers comparing "verbs this ottofile exposes" only ever see ottofile facts.
- **One logical shape in both formats.** JSON and YAML are the same map keyed by task name, mirroring the ottofile's own `tasks:` shape. `jq -r 'keys[]'` and `sed`-ing YAML top-level keys answer the same question.
- **Foreach parents appear once.** A task with a `foreach:` directive appears as a single top-level entry; its expanded subtask ids live in that entry's `subtasks` array. There is no separate top-level entry per subtask.
- **Stdout is pure data.** Nothing but the selected format's bytes reaches stdout. All notices (e.g. a foreach that resolved to zero items) and all errors go to stderr.
- **A resolution failure exits non-zero with nothing on stdout.** If any task's foreach can't be resolved, `--tasks` prints the error to stderr, exits non-zero, and emits no stdout at all rather than a partial or inconsistent map.

## Per-Task Fields

| Field | Type | Description |
|---|---|---|
| `help` | string or null | The task's `help:` text |
| `params` | array | One entry per declared param, sorted by name |
| `params[].name` | string | The param's divined name |
| `params[].flags` | array of strings | CLI flags, e.g. `["-s", "--svc"]` |
| `params[].choices` | array of strings | Static `choices:` values (empty if none). Absent entirely for a `choices-command:` param |
| `params[].choices-command` | string | The param's `choices-command:`, verbatim. Present only for a dynamic-choices param, in place of `choices` |
| `params[].default` | string or null | The param's `default:` value |
| `params[].positional` | bool | `true` for a bare-named (positional) param |
| `edges.before` | array of strings | Task names this task's `before:` edges point at |
| `edges.after` | array of strings | Task names this task's `after:` edges point at |
| `subtasks` | array of strings | Expanded subtask ids (`<task>:<item>`) for a `foreach:` task; empty for an ordinary task |

`choices` and `choices-command` are mutually exclusive in the ottofile, so
exactly one of the two keys appears per param. `--tasks` reports the command
verbatim and never runs it: a surface executes only what it needs, and the task
list needs subtask ids (so a `foreach: command:` does resolve), not the value
set behind a param. Run the printed command yourself to see the current values.

## Example

```yaml
tasks:
  up:
    help: "Build + start each service in scope"
    foreach:
      items: [alpha, beta]
    params:
      -s|--svc:
        help: "service name"
  down:
    help: "Stop each service"
    after: [up]
```

```bash
$ otto --tasks | jq .
```

```json
{
  "down": {
    "help": "Stop each service",
    "params": [],
    "edges": { "before": [], "after": ["up"] },
    "subtasks": []
  },
  "up": {
    "help": "Build + start each service in scope",
    "params": [
      { "name": "svc", "flags": ["-s", "--svc"], "choices": [], "default": null, "positional": false }
    ],
    "edges": { "before": [], "after": [] },
    "subtasks": ["up:alpha", "up:beta"]
  }
}
```

```bash
$ otto --tasks --format yaml
```

```yaml
down:
  help: Stop each service
  params: []
  edges:
    before: []
    after:
    - up
  subtasks: []
up:
  help: Build + start each service in scope
  params:
  - name: svc
    flags:
    - -s
    - --svc
    choices: []
    default: null
    positional: false
  edges:
    before: []
    after: []
  subtasks:
  - up:alpha
  - up:beta
```

## Use Cases

### Doctor scripts (sed-only consumers)

```bash
# Does this ottofile expose the verbs we delegate to it?
otto --tasks --format yaml | sed -n 's/^\([a-zA-Z_][a-zA-Z0-9_-]*\):$/\1/p'
```

### jq consumers

```bash
# List task names
otto --tasks | jq -r 'keys[]'

# Inspect one task
otto --tasks | jq '.up'

# List a foreach task's expanded subtask ids
otto --tasks | jq -r '.up.subtasks[]'
```

## Notes

- `--tasks` resolves `foreach:` sources the same way `--list-subtasks` does, so subtask ids are always the real, current expansion, not a static count.
- That includes a [`foreach: command:`](../foreach-subtasks.md) source: `--tasks` is an enumeration surface, so it runs the command once (reporting the real subtask ids is its job) and a non-zero exit becomes its own loud error with nothing on stdout. `otto --help` still never runs it.
- A `foreach:` that resolves to zero items is not an error: it prints a one-line notice to stderr and the task's `subtasks` array is empty.
- `--tasks` never executes a task's `bash:`/`python:`/`action:` body.

## Related Commands

- `otto --list-subtasks` - human-readable subtask listing (same underlying foreach resolution)
- [`foreach:` subtask generation](../foreach-subtasks.md) - how `subtasks` ids are derived
