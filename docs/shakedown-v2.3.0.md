# CLI Shakedown Report: otto v2.3.0

Binary under test: `otto v2.2.1-51-g8664244` (Cargo version 2.3.0), installed with
`cargo install --path .` from `second-code-review-remediation` at `8664244`.
Run 2026-09-05. Every command below was executed for real; nothing is transcribed
from memory.

Weighted toward what this branch changed: the makefile converter, `Clean` exit
codes, scalar task keys, foreach subtask shorts, env sibling resolution, the
`Upgrade` version ordering, and the `lint` / `docs-check` gates.

## Summary

| Metric | Count |
|--------|-------|
| Commands discovered | 6 builtins + 21 repo tasks + 11 global options |
| Commands tested | 34 invocations |
| Passed | 31 |
| Failed | 0 |
| Findings (cosmetic / UX) | 3 |
| Skipped | 3 (mutating or interactive: `Upgrade` real install, `Upgrade --rollback`, `otto install`) |
| Pipelines tested | 2 |
| Edge cases tested | 5 |

No command crashed, hung, or produced a wrong result.

## What this branch changed, verified through the installed binary

| Change | Invocation | Result |
|--------|-----------|--------|
| Scalar task key resolves its edge | `otto report` with `0x1f:` and `after: [0x1f]` | Both tasks ran; `otto Graph` shows `0x1f` with `report` beneath it |
| Env value waits for a declared sibling | same run | `A_ENDPOINT=vault.example`, resolved from a command that never names `Z_HOST` |
| `${VAR:-default}` inside a command | same run | `GREETING=hello world` |
| foreach subtask claims parent's `-t` | `otto up:gamma -t prod` | `up gamma target=prod`, TUI not triggered |
| foreach subtask claims parent's `-h` | `otto serve:two -h db.internal` | `serve two host=db.internal`, help not intercepted |
| Converter: substitution reference in target | `otto Convert` | `target $(SRCS:.c=.o) is a make expansion; the rule is skipped` |
| Converter: unclosed `$(` | same | `unclosed $( or ${ in $(BAD: headers; the rule is skipped` |
| Converter: expansion prerequisite | same | edge dropped, warning names it |
| Converter: prerequisite with no rule | same | edge dropped, warning explains the whole-file consequence |
| Converted output actually runs | `otto -o converted.yml link` | `[link] linking`, exit 0 |
| `Convert --strict` still refuses warnings | `otto Convert --strict` | exit 1 |
| `Clean` exit code on an empty sweep | `otto Clean --keep-days 0` twice | `No runs matching deletion criteria found`, exit 0 both times |
| `Upgrade` orders a dev build above its tag | `otto Upgrade --dry-run` | `Current version is newer than target version. Use --force to downgrade.` |
| `lint` can fail | `otto lint` | `✅ No trailing whitespace found` on a clean tree; exits 1 on a dirty one |
| `docs-check` runs as its own gate | `otto docs-check` | `48 relative .md links, every one resolves` |

## Command results

**Builtins.** `Clean` (`--dry-run`, `--keep-days`, real delete), `Convert`
(stdin, `--strict`), `Graph` (`ascii`, `dot`), `History` (table, `--json`),
`Stats` (table, `--json`), `Upgrade` (`--dry-run`, `--list-versions`). All exit 0
on success. `Upgrade --list-versions` reached the real GitHub API and listed nine
releases, newest `v2.2.1`.

**Global options.** `--tasks` with `--format yaml` and `--format json`,
`--list-subtasks`, `-o/--ottofile`, `-j/--jobs`, `-t/--tui`, `--help`,
`--version`. `-t` without a tty prints `Warning: --tui requires a TTY, falling
back to standard output` and runs the task rather than failing.

**Repo tasks.** `lint` and `docs-check` run directly. The heavier gates
(`ci`, `cov`, `test`) were exercised repeatedly during the branch's own work and
are not repeated here.

## Output format matrix

| Command | table | `--json` | `--format yaml` | `--format json` | dot |
|---------|-------|----------|-----------------|-----------------|-----|
| `--tasks` | n/a | n/a | works | valid JSON object | n/a |
| `History` | works | valid JSON array | n/a | n/a | n/a |
| `Stats` | works | valid JSON object | n/a | n/a | n/a |
| `Graph` | ascii works | n/a | n/a | n/a | works |

Both JSON surfaces validate through `jq`. `History --json` records carry
`args`, `cwd`, `duration_seconds`, `ended_at`, `hostname`, `id`,
`ottofile_path`, `project_id`, `run_dir`, `size_bytes`, `status`, `timestamp`.

## Findings

None blocking. Three are cosmetic or UX.

**1. `otto Graph --help` prints the `[built-in]` marker; the other five do not.**
`otto Clean --help` opens with `Clean old otto run directories`, but
`otto Graph --help` opens with `[built-in] Visualize the task dependency graph`.
`Graph` is the one builtin deliberately not early-routed in `main.rs` (it needs
the parsed ottofile), so its help comes from otto's own renderer, which adds the
marker, while the other five reach clap directly. Severity: cosmetic. The marker
belongs in the aggregated `otto --help` list, not in a single command's own help.

**2. `otto Upgrade --help` prints maintainer rationale as user help.** The
`--github-token` entry renders three sentences explaining why
`hide_env_values(true)` is set, including the sentence about a token leaking into
scrollback. That is a code comment addressed to whoever edits the struct, and it
is being shown to whoever runs `--help`. Severity: cosmetic.

**3. `${VAR:-default}` fails outside a command substitution.** `GREETING: "hello
${WHO:-world}"` at the top level fails the load with `Environment variable
'WHO:-world' not found`, naming a variable nobody wrote. Inside `$(...)` the same
expression works and yields `hello world`. This is pinned behavior
(`tests/dollar_escape_test.rs`, and the 2026-09-04 amendment for `2b976eb`
states it), so it is a deliberate boundary rather than a regression, but the
error names a reference rather than a variable and does not say that the brace
form is only supported inside a command. Severity: UX suggestion.

## Pipeline recipes

```bash
# Which tasks declare parameters, and how many
otto --tasks --format json | jq -r 'to_entries[] | select(.value.params | length > 0) | "\(.key): \(.value.params | length) param(s)"'
# serve: 1 param(s)
# up: 1 param(s)

# Run outcomes by status
otto History --json | jq -r '.[].status' | sort | uniq -c
#       2 Success
```

## Edge cases

| Input | Behavior | Exit |
|-------|----------|------|
| `otto nosuchtask` | `unknown task 'nosuchtask'` | 1 |
| `otto build --nosuchflag` | clap: `unexpected argument '--nosuchflag' found`, then usage | 1 |
| `otto -o /nonexistent/otto.yml build` | `ottofile path '/nonexistent/otto.yml' does not exist: No such file or directory (os error 2)` | 1 |
| `otto -j 0 build` | `invalid value '0' for '--jobs <N>': 0 is not in 1..` | 1 |
| `otto ""` | rejected | 1 |

Every one fails with a message naming the input, and none panics.

## Release validation

Not applicable yet, and expected: `v2.3.0` is unreleased.

- Tag `v2.3.0`: does not exist. `Cargo.toml` is at 2.3.0 (set by `cfc60e6`), so
  `bump -n` refuses with `Cargo.toml has 2.3.0 but latest git tag is v2.2.1`
  until the tag is cut.
- GitHub release `v2.3.0`: `release not found`. Latest published is `v2.2.1`
  (2026-09-02).
- `v2.2.1` is an annotated tag (`git cat-file -t v2.2.1` reports `tag`).
- No release assets were downloaded, because there is no matching release for the
  version under test.

The tag is blocked on PR #3 merging; `bump --tag-only` refuses off the default
branch by design.

## Observations

- Help is consistent across surfaces for user tasks: `otto up --help` and
  `otto help up` render the same body, and the foreach parent's help carries
  `[3 items]` plus the injected `--Serial` flag.
- The aggregated `otto --help` now shows each builtin's own `about`, so
  `Clean old otto run directories` appears identically there and in
  `otto Clean --help`. That is the fix from `e706e29` visible end to end.
- `otto Graph` renders foreach families compactly (`up:{alpha,beta,gamma}`)
  rather than one line per subtask.
