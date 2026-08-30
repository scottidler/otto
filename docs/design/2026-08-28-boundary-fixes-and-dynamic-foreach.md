# Design Document: Boundary Fixes and Dynamic Foreach

**Author:** Scott A. Idler
**Date:** 2026-08-28
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

The otto/otto-dev boundary review (see References) reproduced three defects, one help-rendering drift, and three feature gaps in otto, all verified against `otto v1.2.6` at `otto@10e9cac`. This doc fixes the four defects and adds four features: `foreach` with a command source (the headline), dynamic param `choices` from a command, a machine-readable task list, and opt-in per-task tty. On the otto-dev side, dynamic foreach can replace the generated per-service YAML region (408 lines) and the ordering/scheduling half of `stack.sh`; what stays there, per the boundary review itself, is scope resolution, profile policy, and the completion messaging that only otto-dev can phrase. Everything lands in otto; otto-dev is untouched and adopts on its own schedule.

## Problem Statement

### Background

- A downstream work repo composes seven internal service repos over otto. It is otto's most demanding consumer.
- otto's task set is fixed when the YAML parses; otto-dev's is computed per developer, per invocation. The gap is bridged today by a 245-line YAML generator emitting 408 lines of `.otto.yml`, plus a 358-line hand-rolled scheduler (`stack.sh`).
- Every finding below was reproduced at a terminal on 2026-08-28 against installed `otto v1.2.6`, HEAD `10e9cac`. Root causes traced to source lines.
- Spec source: the boundary review (References). Requirements below trace to it and to the handoff artifact.

Two corrections to the boundary review, found during design research:

- **Task stdin already IS a tty** under a real pty (verified with `script -qec`). Only stdout/stderr are piped (`src/executor/scheduler.rs:938-940`); there is no stdin redirection anywhere in src/. The review measured from a non-tty context. The tty feature is therefore smaller than the review implies: inherit stdout/stderr, skip capture/prefix, run exclusively.
- **Positional arguments already work.** `ParamType::POS` is wired end-to-end (`src/cfg/param.rs:342` makes bare-named params positional; `src/cli/parser.rs:1376` builds the clap positional). Verified: a task with `params: {svc: {help: ...}}` accepts `otto sw web` -> `svc=web`. otto-dev's failure means the task didn't declare one. Reframed as documentation (Phase 8), with one real sharp edge to document: `partitions()` splits args on task names, so a positional value that collides with a task name mis-partitions.

### Problem

Four defects, all reproduced:

1. **An env can't read its own inherited value.** `src/cfg/env.rs:21-25` strips every declared key from `current_env` before any expression evaluates; the `$()` subprocess gets `env_clear()` + essentials + that stripped context (`env.rs:129-139`), so `${MYVAR:-fallback}` sees nothing. The ordinary "declare a default, let the shell override it" idiom can't be written under its own name. (**Amended 2026-08-29, implementation audit:** a reader copying the bare form from this line gets an error even after Phase 2. otto's own substitution treats `MYVAR:-fallback` as a variable *name*: `Failed to resolve environment variable 'MYVAR': Environment variable 'MYVAR:-fallback' not found`. The working idiom is to let `sh` do the defaulting: `MYVAR: '$(echo "${MYVAR:-fallback}")'`, which is what the Acceptance Criteria fixture and `tests/env_self_reference_test.rs` use. Phase 2 fixed self-reference, not bare-`${VAR:-default}` parsing; the latter is untouched and remains remediation-doc scope.) This is what forced otto-dev's `<SVC>_ROOT` naming workaround.
2. **`$(...)` truncates at the first `)` instead of the matching one.** `Regex::new(r"\$\(([^)]+)\)")` at `src/cfg/env.rs:100`; `[^)]+` can't nest. Observed side effect: the failed substitution warns and abandons the entire global env map (a sibling `MYVAR` in the same fixture came through unbound).
3. **Serial `foreach` subtask targeting pulls in every preceding sibling.** `src/cli/parser.rs:1227-1232` implements serial ordering by pushing real `before:` edges from each subtask to its predecessor; `collect_transitive_deps` (`parser.rs:1251-1289`) then walks them. "Runs after" and "requires" are the same edge, so `otto up:gamma` under `parallel: false` runs alpha, beta, gamma. Under `parallel: true` it runs exactly gamma.
4. **`-C/--cwd`, `-o/--ottofile`, `--list-subtasks` are absent from `otto --help`.** Registered in `otto_command()` (`parser.rs:486-532`), but the rendered help comes from `build_help_command()` (`parser.rs:1388-1441`) and `build_help_command_with_error()` (`parser.rs:1443-1465`), which re-declare only `jobs` and `tui`. The `-C` design doc (`docs/design/2026-03-26-directory-flag.md`, Implemented) specifically claims `-C` appears in help. The builders drifted.

Three feature gaps:

5. **`foreach` accepts only `glob`, `items`, or `range`** (`src/cfg/task.rs:47-73`, `resolve_items` at `:86-118`). No way to expand a task over a runtime-computed list. This single gap is why otto-dev carries the generator and the second scheduler.
6. **No machine-readable task list.** otto-dev's doctor scrapes `otto --help` with a sed regex to answer "does this repo expose the verbs we delegate to it."
7. **Interactivity has nowhere to live.** Output is prefixed and captured, so every interactive flow (credential prompts, pickers) sits outside the task graph in scripts that check for a tty themselves.
8. **Param `choices` must be a static list** (`src/cfg/param.rs:82`, wired to clap at `parser.rs:1368-1370`, re-validated on propagation at `:936-946`). Validation is good when it fires, but registry-derived value sets can't use it, forcing N generated tasks where one parameterized task should do.

### Goals

- Fix all four defects with regression tests in the same phase.
- `foreach: command:` expanding one task over a runtime-computed item list, with the load-time-execution tension weighed explicitly (not treated as settled).
- `otto --tasks`: machine-readable JSON task list (name, help, params, edges, subtask ids).
- `choices-command`: param choices sourced from a command, validated at bind time, never executed for help (requested by Scott, 2026-08-28, reversing the initial parking).
- `tty: true` per task: hand the task the terminal's output, don't prefix, don't capture, run exclusively.
- Fix defect 3 before shipping `foreach: command:` (sequential bring-up + subtask targeting is exactly the combination that breaks).

### Non-Goals

- Any change to otto-dev. Different repo, different owner.
- Moving any Tatari-specific fact (aws-vault profiles, ports, compose projects) into otto. The review concluded the conceptual boundary is correct.
- Bootstrap/cloning/layout-conversion capabilities. A task runner should not grow a package manager.
- The remaining scope of the two 2026-06-10 in-review docs (see next section). Only the bullets this doc supersedes are touched there.

## Relationship to In-Flight Docs (ship order)

Two uncommitted docs, both Status: In Review, no implementation branches, nothing landed:

- `docs/design/2026-06-10-code-review-remediation.md` claims two of this doc's items: the help-drift fix (its Phase 8 `parser.rs:1388-1465` bullet) and the `$()` nesting fix (its Phase 3 remainder bullet, "paren-balancing scan or document"). Its Phase 2 serial-foreach mid-chain-failure bullet assumes the current chain-edge implementation.
- `docs/design/2026-06-10-architecture-product-completion.md` (sequenced after remediation) plans `--list-tasks` and `--plan --format json`, and moves `$()` execution out of load time (stage 2 "NO subprocess execution").

**Decision: this doc ships first.** It is small, every item is reproduced, and it is driven by a live consumer. The two 06-10 docs are large and unbuilt after 2.5 months; blocking on them is blocking forever. De-duplication (Phase 0) so no two docs claim the same fix:

- Remediation Phase 8 help-drift bullet -> "covered by 2026-08-28 doc Phase 1".
- Remediation Phase 3 `$()` nesting sub-bullet -> "covered by 2026-08-28 doc Phase 3". The rest of its Phase 3 env work (placeholder map, bare-`$VAR` guard deletion) stays there and rebases onto this doc's env.rs changes.
- Remediation Phase 2 serial-foreach bullet -> note that this doc's Phase 4 redesigns serial foreach as a scheduler property; the mid-chain-failure semantics decided here (skip successors) are what its test spec should pin.
- Remediation Phase 8 `-C=DIR` bullet (its line 242) -> "covered by 2026-08-28 doc Phase 1" (`apply_cwd_flag` learns the attached form there).
- Explicitly NOT moved: remediation's `-j 0` fix (its line 158; the hang fires at `scheduler.rs:397` before tty behavior is reachable, so Phase 7 only states the precondition), its help-path `-o/$OTTOFILE` honoring (its line 238; that bug lives in the ottofile divining at `parser.rs:307-309`, not in the arg-declaration code Phase 1 touches at `:486-532`/`:1388-1465`), and its token-boundary-collision lookahead fix (its line 241; this doc's Phase 8 only documents the edge).
- Architecture doc -> note that `--tasks` (this doc) is the near-term machine-readable surface; when the staged pipeline lands, `--tasks` re-implements over the pipeline's TaskGraph or is subsumed by `--plan --format json`, and `foreach: command:` resolution moves into its expansion stage under the same lazy rules (see Phase 6 tension paragraph).

Cross-repo blast radius: none. All changes in otto. otto-dev can later delete its generator and scheduler, on its own schedule, no coordinated ship. Within otto the forced order is: this doc -> remediation -> architecture.

## Proposed Solution

### Overview

Nine phases plus a docs-only Phase 0 (Phase 6b rides Phase 6's resolver and lands directly after it). Deterministic and cheap first (help drift), then the two env.rs fixes, then the serial-foreach scheduler change (must precede dynamic foreach), then the four features, then the small items. Every phase lands with regression tests and `otto ci` green, one commit each.

### Architecture

Per item:

**1. Help drift (Phase 1).** One `fn global_args() -> Vec<Arg>` declaring cwd/ottofile/list-subtasks/jobs/tui; `otto_command()`, `build_help_command()`, `build_help_command_with_error()` all consume it. Reconciliation with remediation's "remove the dead parser-side cwd Arg" finding (`-C=DIR` silently swallowed by clap today): the Arg stays registered (it is what renders help, and single-sourcing is the point), and `apply_cwd_flag` in main.rs learns the attached `-C=DIR` form so clap never sees any `-C` variant. A help snapshot test pins the output so the builders can't drift again.

**2. Env self-reference (Phase 2).** In `evaluate_envs` (`src/cfg/env.rs:8-75`), build the evaluation context per expression, not once: (system env minus all declared keys) + already-resolved declared values + {the key under evaluation: its inherited value}. Self-reference resolves to the inherited value; a reference to a different not-yet-resolved declared key still defers (the "Environment variable not found" retry at `env.rs:41-44` is preserved); later envs see the declared value once resolved. The hazard this design avoids: seeding inherited values for ALL declared keys would let a cross-reference resolve prematurely to the inherited value, nondeterministically (HashMap iteration order). `MAX_ITERATIONS` still covers the circular case. One fix covers all three scopes: global (`parser.rs:706`), task (`parser.rs:186`), merged (`executor/task.rs:156`).

Recorded adjacent latent bug (not fixed here, noted for the remediation doc's env work): inside `$()`, a reference to an unresolved declared key expands to empty in sh rather than deferring; cross-refs inside `$()` are already order-sensitive today. The deferral mechanism only works for `${VAR}` outside `$()`.

**3. `$()` matching (Phase 3).** Replace the regex find in `resolve_shell_commands_with_env` (`env.rs:95-113`) with a scanner that finds `$(` and walks to the matching `)` counting depth, tracking single quotes, double quotes, and backslash escapes so a quoted `)` doesn't close the substitution. The rest of env resolution stays regex-based. `$(echo ")")` yields `)`. An unmatched `$(` (no closing paren before end of value) is a loud config error naming the key and the value, never a silent literal pass-through.

**Amended 2026-08-29 (doc defect: this bullet contradicted the doc's own scoping).** "Loud config error" is satisfied in the sense the phase can deliver: `evaluate_envs` *raises* an error naming the key and the value, and the silent literal pass-through is gone. It is NOT fatal, because all three call sites demote every env-evaluation error to a warning and continue (`src/cli/parser.rs:712` global, `src/cli/parser.rs:159` and `src/executor/task.rs:122` task scope), each `eprintln!`-ing and substituting an empty map. Making it fatal is a change to that call-site policy, which applies to *every* env failure including `$(exit 3)` — and that is precisely defect 2's recorded whole-map-abandonment side effect, which this doc's Phase 3 entry explicitly assigns to the remediation doc. The bullet as originally written demanded a fatality the phase was simultaneously forbidden to implement. Observed after Phase 3 (`10eb334`), fixture `BROKEN: '$(echo hello'` + `GOOD: 'still-here'`: `Warning: Failed to evaluate global environment variables: Environment variable 'BROKEN': unmatched '$(' (no closing ')') in value '$(echo hello'`, then `[show] BROKEN=[UNSET] GOOD=[UNSET]`, exit 0 (exit 1 if the task reads the key without a shell default, since generated scripts run under `set -u`). For contrast at `c026a7d`: no env diagnostic at all, the literal reached the generated script and produced `unexpected EOF while looking for matching '"'`, exit 1. **Deferred, not dropped:** whether an env-evaluation failure should be fatal at all is one decision for all env failures, and it belongs to the remediation doc's abandonment work. Deliberately rejected here: making unmatched-`$(` alone fatal, which would leave two different failure policies inside one function.

**4. Serial foreach as a scheduler property (Phase 4).** Parser stops pushing chain `before:` edges (`parser.rs:1227-1232`). Executor `Task` (which already carries `parent: Option<String>`, `src/executor/task.rs:41`) gains a serial-group membership + order index. The scheduler's ready loop (`scheduler.rs:362-462`) applies one rule per serial-group member, where "predecessor" means the nearest preceding group member **that is in the run set**:

- no predecessor in the run set -> eligible immediately
- predecessor in `completed_set` (Completed, or up-to-date-skipped, which already lands there at the `scheduler.rs:296` region) -> eligible
- predecessor in any other terminal state (`failed_set`, or `skipped_set` for ANY reason: an ordinary `when:` edge going Unreachable at `scheduler.rs:420-426`, or a group cascade) -> this member is Skipped with a visible reason, entering `skipped_set` and `skip_reasons` exactly like the existing Unreachable path, so the cascade propagates through the same rule

The rules classify by the scheduler's existing terminal sets, so every terminal state of a predecessor is covered and the gate can never leave a successor waiting forever (panel round 2 counter-flaw, conceded and folded: a predecessor skipped by an ordinary conditional edge is in `skipped_set` only, and skipping the successor there matches what the current `when: success` chain edges do today). No new provenance tracking is needed: cascade skips reuse `skipped_set`. The gate composes with, never replaces, ordinary dependency readiness: a member with its own `before:` edges must satisfy both. Consequences:

- Targeting `up:gamma` schedules only gamma; no predecessors in the run set, gate trivially satisfied. Ordering constrains the run set; it never expands it.
- Full run `otto up`: subtasks run in declared order, never concurrently.
- **Failure semantics (decided): a failed predecessor skips the remaining group members** (the third gate rule above). Same observable behavior as today's `when: success` chain when the parent runs. Rationale: serial bring-up is the use case, and starting gamma after alpha failed is exactly wrong for a stack; it also keeps the remediation Phase 2 and architecture-doc timeout test specs valid. The virtual parent aggregates as today.
- Both `parallel: false` in config and the `--Serial` CLI flag flow through the same `run_serial` at `parser.rs:1199`; one fix covers both.
- Why not "ordering-only edges": the `task_deps` override at `parser.rs:799` copies edges unfiltered, and the scheduler's dep check (`scheduler.rs:809-836`) hard-errors on a dep that was never scheduled; ordering-only edges would need pruning at every consumer. The group property's consumers are the ready loop plus two known chain-edge dependents handled explicitly below.
- Skip provenance: no `SkipKind` refactor is needed here because the gate never reads `TaskStatus::Skipped` to decide; it classifies by the existing `completed_set` / `failed_set` / `skipped_set`, which already separate up-to-date skips (in `completed_set`) from every other skip (in `skipped_set`). The full `SkipKind` refactor stays in the remediation doc for its own bugs.
- Aggregation: ~~verified: the virtual parent is failed-first (`scheduler.rs:545-549`: any Failed subtask -> parent Failed, before any Skipped check), so alpha failing with beta/gamma cascade-skipped aggregates the parent to Failed with no aggregation change.~~ **Amended 2026-08-29 (doc defect: this bullet was marked "verified" and was not).** The parent aggregates to **Skipped**, not Failed, and did so before this doc was written. The failed-first branch at `scheduler.rs:545-549` is unreachable for this case: `classify_edge` short-circuits at `src/executor/scheduler.rs:176` with `if skipped.contains(&edge.task) { return EdgeState::Unreachable; }`, ABOVE the `when:` match, so a Skipped source is Unreachable for every `when:` variant including `always`. The virtual parent's edges point at the cascade-skipped subtasks, so the parent never executes and never aggregates. Measured on the alpha-fails fixture, byte-identical at `10eb334` (pre-Phase-4) and `121f384` (post): `STATUS up = Skipped`, `STATUS up:alpha = Failed(..)`, `STATUS up:beta = Skipped`, `STATUS up:gamma = Skipped`, `RUN_RESULT_IS_ERR=true`, exit 1 on both. Phase 4 therefore preserved the behavior (this bullet's other, correct instruction was that aggregation does not change) and pinned `Skipped` in its test. **Consequence for the reader:** the doc's claim that the parent goes Failed was never true; the exit code is non-zero regardless, sourced from the failed subtask, so nothing downstream of exit-code checking is affected. Whether a virtual parent with Failed + Skipped subtasks *should* report Failed is an aggregation change and is remediation-doc scope. The all-up-to-date-Skipped downstream bug stays remediation Phase 2 scope.
- Known chain-edge dependents (verified, panel rounds 1-2): the ASCII Graph view filters internal `parent:` edges out of its collapsed display (`graph.rs:431-437`), so it is unchanged. The DOT family (`dot`, `svg`, `png`, `pdf`, and the default `Auto`) walks raw `task_deps` in `generate_dot` (`graph.rs:311-320`) and DOES render the chain edges today; this phase makes `generate_dot` emit serial-group ordering as dashed edges labeled `order` between consecutive group members (derived from the group order index, visually distinct from `depends` edges). Disclosed output change: serial subtask chains in DOT output change label `depends` -> `order`; ordering visibility is preserved, and the edge now tells the truth about what it is. Two tests assert chain edges (`tests/flag_integration_test.rs:404` region, `src/cli/parser.rs:2652` region) and are rewritten in this phase to assert group ordering instead (disclosed test change, they must still bite).

**5. `--tasks` (Phase 5).** Global flag beside `--list-subtasks` (the exact handling pattern, `parser.rs:389-393`): after config load and foreach expansion, emit the task list and exit 0, executing no task. Output follows the full house rule (taste.md: "TTY-detect output: yaml for humans, json when piped, one `--format` override, no boolean format flags"): stdout a tty -> YAML; piped -> JSON; `--format {yaml,json}` overrides. History/Stats' boolean `--json` (`parser.rs:1700, 1779`) is the legacy shape this deliberately does not copy; Graph's `--format` (`:1482`) is the precedent it does. The piped-JSON default means machine consumers write `otto --tasks | jq ...` with no flag at all, and a `jq`-less consumer (otto-dev's doctor is sed-only, verified) uses `--format yaml` and seds top-level keys. JSON shape (documented in `docs/commands/`):

One logical shape in both formats (panel round 2 flag, resolved): a map keyed by task name, mirroring the ottofile's own `tasks:` shape. YAML's top-level keys are the task names (sed-able); JSON is the same map (`jq -r 'keys[]'` for names, `jq '.up'` for one task):

```json
{
  "up": {
    "help": "Build + start each service in scope",
    "params": [{"name": "svc", "flags": ["-s", "--svc"], "choices": [], "default": null, "positional": false}],
    "edges": {"before": ["deps"], "after": []},
    "subtasks": ["up:alpha", "up:beta"]
  }
}
```

A dedicated serde view, not `TaskSpec`'s custom `Serialize` (that one is shaped for YAML round-trip fidelity, wrong contract for consumers). `serde_json` is already a direct dep. Enumeration surfaces (`--tasks`, `--list-subtasks`) DO resolve command-sourced foreach (below): giving the real list is their job, and that is documented.

Contract, frozen (review finding, folded): user-defined ottofile tasks only, no injected builtins (consumers compare ottofile verbs, and builtins are not ottofile facts); foreach parents appear once with their `subtasks` array (no separate top-level subtask entries); the YAML form carries the same fields; stdout is pure data in the selected format, all notices and errors go to stderr; a command-source resolution failure exits non-zero with nothing on stdout.

**6. `foreach: command:` (Phase 6).** New `ForeachSpec` source, mutually exclusive with `glob`/`items`/`range` (loud config error if combined, matching the existing "foreach requires glob, items, or range" error site at `task.rs:94`):

```yaml
up:
  foreach:
    command: "scripts/list-services.sh"   # illustrative; the consumer owns its command
    as: svc
    parallel: false
  bash: |
    scripts/svc.sh run "${svc}" up
```

Semantics:

- The command runs via `sh -c`, cwd = the ottofile's directory, env = inherited environment + resolved global `envs:` (ordering verified: global envs evaluate at `parser.rs:706`, before expansion at `:716`). Task params are NOT available (params resolve after expansion, `parser.rs:731-834`); the doc for the feature states this plainly. Dynamic input comes from the shell env or a file the command reads.
- Items = non-empty lines of stdout, whitespace-trimmed. Identifiers sanitized exactly as glob identifiers are (`task.rs:139`); `max_items` (default 1000) and the duplicate-identifier error (`task.rs:478-487`) apply unchanged.
- Non-zero exit = loud config error naming the task and the command, never a silent zero-subtask expansion. Zero lines with exit 0 = zero subtasks plus a one-line notice on stderr (an empty scope is a legitimate state for the consumer).
- Ordering constraint for the implementation: partition-time task-name matching (`get_task_names`, `parser.rs:683`) currently runs BEFORE global env evaluation (`:706`). If a subtask-shaped token triggers resolution at partition time, the command would see unresolved global envs. The phase moves global env evaluation ahead of partitioning (global envs do not depend on task args, so the move is safe) so the env contract above holds at every resolution site.
- **Lazy, cached once per invocation.** Today `resolve_items` runs unconditionally at FOUR production call sites (review finding, verified): expansion (`task.rs:475`), arg partitioning (`parser.rs:683`), `--list-subtasks` (`parser.rs:619`), and help item-count rendering (`parser.rs:1303`). A command source resolves only when needed: (a) the requested args include the parent task or a subtask-shaped token (`up:gamma` when `up` is the command-sourced foreach), including via default-task `*` expansion, (b) an enumeration surface runs: `--tasks`, `--list-subtasks`, and `otto Graph` (**amended 2026-08-29, implementation audit:** `Graph` was missing from this list, which read as a closed enumeration. `parse_all_tasks` in `src/executor/graph.rs` passes every task name to the reachability filter, so the defer is a no-op there and a command source resolves. Measured at `4ff3206` with a counter fixture: `Graph` 1, `build` 0, `--help` 0 -- so the promise below about an unrelated task, and the `--help` promise, both hold; only this list was incomplete). Those surfaces propagate a resolution failure as their own loud error. Resolution happens at most once per invocation (cached). `otto --help` never executes it: the help item count for a command-sourced foreach renders as `[dynamic]` instead of `[N items]`, replacing the `parser.rs:1303` call. `otto build` in a repo whose unrelated `up` task has a command source never executes it.
- Resolver shape (review finding, folded): this is not a free move. `get_task_names` (`parser.rs:675`) takes `&self` only and silently ignores resolution errors (`let Ok(items)` at `:683`); silent-ignore is unacceptable for a command source (loud failure is the contract). The phase introduces a per-invocation resolution cache on the `Parser` (interior-mutable side cache or resolver object) threaded to the partition/list/expand sites, and `get_task_names` gains access to the requested args and the resolved global envs so the lazy trigger can be evaluated. Glob/items/range behavior is untouched.
- Phase coupling (panel finding, folded): the `[dynamic]` help rendering lands in `task_to_command_for_help` (`parser.rs:1303`), which is reached from `build_help_command` (`:1421`, `:1434`), the function Phase 1 rewrites. Phase 1 lands first; Phase 6 edits the successor function, and the Phase 1 help snapshot test pins the seam between them.
- Recursion guard (review question, answered): the command runs with `OTTO_FOREACH_COMMAND=<task>` set in its environment. If an inner otto invocation would resolve a command source for a task already named in that variable, it errors loudly (cycle named in the message) instead of recursing. An inner otto that targets ordinary tasks is unaffected. A subtask-shaped token that fails to validate after resolution (`up:nosuch`) follows existing unknown-task behavior; triggering a resolution merely to validate a token is correct, that is what validation costs.

The design tension, weighed: `docs/foreach-subtasks.md:56` lists "runtime code execution" as a non-goal, and the architecture doc's staged pipeline removes subprocess execution from load (its problem #3: "otto --help against an untrusted ottofile executes commands"). The counter-argument: otto already shells out at load time for every `envs:` `$()`, unconditionally, on every invocation including `--help`. This feature is lazier than the existing posture, not looser: it adds an execution site that fires only when the invocation actually needs the expansion, and `--help` stays execution-free for it. When the architecture doc's pipeline lands, command resolution moves into its expansion stage under the same lazy rules, and `--plan` renders a command-sourced foreach either unexpanded (`up: dynamic (command: ...)`) or expanded, depending on whether the plan invocation resolved it; that call belongs to the architecture doc and is noted there (Phase 0). `docs/foreach-subtasks.md`'s non-goal is amended with this rationale in Phase 6, not silently contradicted.

**6b. Dynamic param `choices` (Phase 6b).** New `ParamSpec` field `choices-command: Option<String>` (needs an explicit `#[serde(rename = "choices-command")]`; ParamSpec has no struct-level kebab rename), mutually exclusive with a non-empty `choices`: enforced inside `deserialize_param_map` (`param.rs:309`) after `divine()` sets the param identity, loud error naming the param. Rides Phase 6's per-invocation resolver cache.

- **The bind/help seam must be built, not assumed** (panel round 4, both seats): today `task_to_command` (`parser.rs:1292`) just delegates to `task_to_command_for_help(spec, None)`, and `cwd: Option<&Path>` is the implicit mode flag (help callers pass `Some(base_dir)` at `:596`, `:1421`, `:1434` to render foreach item counts; the bind caller at `:762` passes `None`). Phase 6b's bind path needs cwd + the resolver, which inverts that discriminator. The phase replaces it with an explicit builder mode (Help vs Bind carrying resolver/cwd access, methods on `Parser` rather than static fns). Phase 6's `[dynamic]` change already removes help's only use of `cwd` here, so the mode enum completes that cleanup; Phases 1, 6, and 6b all touch this construction path, in that order.
- **Bind path executes, help path never does.** Resolution triggers when a dynamic-choices param is actually validated: direct invocation, or a PROPAGATED value hitting the choices check at `parser.rs:936-946` (invoking task A can execute dependency B's `choices-command` when B is in the run set and receives A's value: intended, B's params must validate, and the `task:param` cache suppresses duplicates). Values feed `PossibleValuesParser` (`:1368-1370`). Help (`otto --help`, `otto help <task>`, `<task> --help`) renders `[dynamic choices: <command>]` and executes nothing: same posture as foreach's `[dynamic]`, one rule for both features (dynamic sources never execute for help, always for use).
- Values = non-empty lines of stdout, whitespace-trimmed, same parsing as `foreach: command:`. Non-zero exit = loud error naming task, param, and command. **Zero lines = loud config error too** (unlike foreach's legitimate empty scope: a param whose valid set is empty can accept no value, which is a misconfiguration, and fail-closed beats accept-anything).
- `--tasks` reports provenance without executing: a dynamic-choices param carries `"choices-command": "<cmd>"` in place of `"choices"`. Rule stays "a surface executes only what it needs": `--tasks` needs subtask ids (so foreach resolves) but has no need of choice values.
- Recursion guard symmetric with foreach: `OTTO_CHOICES_COMMAND=<task:param>` set during execution; a nested otto resolving the same key errors loudly.
- Command context identical to `foreach: command:`: `sh -c`, cwd = ottofile directory, inherited env + resolved global `envs:`, task params unavailable.
- Round-trip pinned (panel round 4): a `choices-command:` param must re-serialize verbatim. This is the test that catches a deserialize-only `#[serde(rename)]`; `tests/roundtrip.rs` already exercises `choices:` (its `:126` region), and ParamSpec round-trip is an actively guarded invariant, so the sibling case is a few lines in an existing file.
- Scope note: `nargs` is inert for user params today (declared at `param.rs:84`, never wired to `num_args`, bind reads `get_one::<String>()` at `parser.rs:783`; fixing it is remediation Phase 6 scope). `choices-command` therefore adds no multi-value validation; each single bound value validates against the dynamic set, same as static `choices`.

**7. `tty: true` (Phase 7).** New `TaskSpec` field, serde-defaulted false. For a tty task:

- Spawn with `Stdio::inherit()` for stdout/stderr (stdin already inherits); skip `TaskStreams` entirely: no capture, no `[task]` prefix.
- Exclusivity: acquire the whole semaphore (`acquire_many(max_parallel)`) at the acquisition point (`scheduler.rs:793`). No concurrent task while a tty task runs; tasks before and after schedule normally. Tokio's semaphore is FIFO, so a waiting `acquire_many` cannot be starved by later single-permit acquires (review-verified).
- `TaskScheduler` gains a `max_parallel` field (review finding, verified: only the constructed `Semaphore` survives `new()`, and reading `available_permits()` at acquisition time would be wrong).
- Precondition, stated not fixed (panel round 1): `-j 0` already hangs every run at the launch loop (`scheduler.rs:397`) before any tty behavior is reachable (verified: `timeout 5 otto -j 0 up` exits 124 with zero output), so the `-j 0` fix is remediation scope, not a prerequisite here. This phase asserts its own precondition instead: `acquire_many` requests exactly the initial permit count, with a debug assertion.
- `--tui` + any tty task in the run set = loud error at the `src/app.rs:235` fork, before anything runs. Documented, deliberate trade.
- `tty: true` on a foreach task applies to each subtask; under `parallel: true` the exclusivity gate serializes them anyway (documented, not an error: the field means "give each of these the terminal").
- History: the stdout/stderr log paths recorded at task start (`scheduler.rs:897-921`) point at files containing a single marker line `otto: tty task, output not captured`, so an empty log never lies about a silent task.

**8. Small items (Phase 8).**

- **Nested prefix:** a `--no-prefix` global flag suppressing the `[task]` prefix (`src/executor/output.rs:98-106`). Opt-in, no auto-detection: keying off `OTTO_TASK` in the environment would silently change output shape when otto runs inside otto, and an inner run with parallel tasks would lose attribution with no way back. otto-dev's wrapper adds `--no-prefix` where it wants it. (Auto-detection recorded as the rejected alternative.)
- **Positional args:** documentation only; the capability exists. Document the declaration shape and the task-name-collision sharp edge in `docs/commands/` and the README examples.

### Data Model

- `ForeachSpec` += `command: Option<String>` (kebab already fine), exclusive with the other three sources; round-trip serialization support like the existing fields.
- `ParamSpec` += `choices-command: Option<String>` (kebab-case), exclusive with non-empty `choices`; round-trip support.
- `TaskSpec` += `tty: Option<bool>` (default false), round-trip support.
- executor `Task` += serial-group name + order index (typed fields, not env-string parsing; `OTTO_FOREACH_INDEX` stays as the env-facing value).
- `TaskScheduler` += `max_parallel: usize` (today only the constructed `Semaphore` survives; `acquire_many` needs the original count).
- `Parser` += per-invocation foreach-resolution cache (interior-mutable; command sources resolve at most once).

### API Design

- `otto --tasks` -> task list on stdout (TTY-detect: YAML on a tty, JSON when piped; `--format` overrides), exit 0, no execution (except command-sourced foreach resolution, documented).
- `otto --no-prefix <tasks...>` -> raw task output, no `[task]` prefixes.
- `.otto.yml`: `foreach.command`, `params.<p>.choices-command`, `task.tty` as above.
- No changes to any existing flag's behavior; `--help` gains lines it should always have had.

### Implementation Plan

#### Phase 0: De-duplicate the in-flight docs
**Model:** sonnet
- Amend the two 2026-06-10 docs per "Relationship to In-Flight Docs": mark the two superseded bullets, add the serial-foreach semantics note and the `--tasks`/`--plan` + foreach-command notes. Both files are uncommitted working-tree drafts; edits land in place, no commit of those files rides this plan.
- **Success criteria:** (a) `rg -c 'covered by docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach' docs/design/2026-06-10-code-review-remediation.md` returns >= 3 (help drift, `$()` nesting, `-C=DIR`) and the same literal marker appears >= 1 time in the architecture doc; (b) manual check recorded with the change: neither doc still claims the three moved items as its own work, and the explicitly-NOT-moved items above are annotated as staying.
  `Observed on main:` zero matches, exit 1 (no cross-references exist yet).

#### Phase 1: Help drift
**Model:** sonnet
- `fn global_args() -> Vec<Arg>`; three builders consume it; `apply_cwd_flag` learns `-C=DIR`; help snapshot test.
- **Success criteria:** (a) `otto --help` lists `-C/--cwd`, `-o/--ottofile`, `--list-subtasks`; (b) all three builders call `global_args()`: `rg -c 'global_args' src/cli/parser.rs` returns >= 4 (one definition + three call sites).
  `Observed on main:` zero matches, exit 1 (the function does not exist yet); (c) snapshot test fails when a builder drifts (demonstrated by breaking it once).

#### Phase 2: Env self-reference
**Model:** opus
- Per-expression context seeding in `evaluate_envs`; regression tests: self-reference, cross-reference deferral under adversarial insertion order (property test over key orderings), circular still errors.
- **Success criteria:** (a) `MYVAR=from-shell otto show` on the repro fixture prints `MYVAR=[from-shell]`; (b) property test: declared cross-refs always resolve to declared values regardless of map order; (c) circular self-definition still errors loudly, with `MAX_ITERATIONS` intact as the backstop. (**Amended 2026-08-29, doc defect:** the original wording said the circular case errors *via* `MAX_ITERATIONS`. It does not, on main or after Phase 2: the error comes from the no-progress fallback branch at `src/cfg/env.rs:59-74`, which fires first; `MAX_ITERATIONS` at `src/cfg/env.rs:15/38/79` remains the backstop for cases the fallback misses. The criterion's behavior assert is unchanged and passes; only the named mechanism was wrong.)
  `Observed on main:` (c) fixture `A: '${B}'`, `B: '${A}'` warns `Failed to evaluate global environment variables: Failed to resolve environment variable 'A': Environment variable 'B' not found` and the task fails on the unbound var, exit 1. Phase 2 must keep this loud.

#### Phase 3: `$()` depth scan
**Model:** opus
- Quote-aware depth-counting scanner replaces the regex find in `resolve_shell_commands_with_env`; all existing env tests stay green.
- **Success criteria:** (a) `NESTED: '$(echo "$(basename /a/b)")'` resolves to `b`; (b) `$(echo ")")` resolves to `)`; (c) `cargo test env` green with zero existing-test modifications.
  `Observed on main:` (b) fixture `PARENQ: '$(echo ")")'` warns `Command 'echo "' failed with exit code 2: sh: 1: Syntax error: Unterminated quoted string`, leaves `PARENQ` unbound, task exits 1. (c) `cargo test env` on main: 13 tests, 12 passed + 1 passed across two binaries, 0 failed (the green baseline Phase 3 must preserve).

#### Phase 4: Serial foreach as a scheduler property
**Model:** opus
- Remove chain edges; group + order on executor `Task`; ready-loop gate on nearest preceding in-run-set member; internal group-skip tracking (no SkipKind refactor); parent aggregation unchanged (verified failed-first); covers both `parallel: false` and `--Serial`.
- Rewrite the two chain-edge tests (`tests/flag_integration_test.rs` serial section, `src/cli/parser.rs` `test_foreach_subtasks_chained_when_parallel_false`) to assert group ordering; implement the DOT-family `order` edges per the Architecture bullet (ASCII untouched).
- **Success criteria:** (a) `otto up:gamma` on the serial fixture runs exactly gamma; (b) `otto up` runs alpha, beta, gamma in order, never concurrently (interleave test); (c) terminal-state matrix integration test: alpha fails -> beta, gamma report skipped with reason, parent aggregates ~~Failed~~ **Skipped** (**amended 2026-08-29, doc defect:** see the Architecture aggregation bullet; the parent is Skipped at `10eb334` and `121f384` alike, because `classify_edge` short-circuits Skipped sources to Unreachable at `src/executor/scheduler.rs:176` before the `when:` match, so the failed-first branch is unreachable and the parent never executes; the criterion asserted an outcome that was false on `main` while the same phase was instructed not to change aggregation), exit non-zero; an up-to-date-skipped predecessor does NOT block its successor; a predecessor skipped via an ordinary `when:` edge (panel fixtures: `before: [dep]` with dep failing, and a failing task with `after: [up:alpha]`) cascades the skip visibly instead of hanging, AND that dep-fails run exits non-zero because dep Failed.
  `Observed on main:` the dep-fails fixture prints `[dep] failed` / `Task dep failed with exit code Some(1)` and exits 1 (the panel's round-2 claim of exit 0 did not reproduce); the three subtasks and the parent are skipped with zero output. Phase 4's criterion pins exit non-zero AND visible skip reasons for the group members it skips.

#### Phase 5: `--tasks`
**Model:** sonnet
- Flag + dedicated serde view + TTY-detect with `--format {yaml,json}` + docs/commands/ entry documenting the frozen contract; sentinel test proves no task executes.
- **Success criteria:** (a) `otto --tasks | jq -e 'type == "object" and (keys | length > 0)'` exits 0 (piped default is JSON), and `otto --tasks --format yaml` emits YAML whose top-level keys are the same task names (one logical shape, two encodings); (b) subtask ids (`up:alpha`) present in `subtasks` arrays AND no builtin (Clean/History/...) keys appear; (c) sentinel test: no task body runs during `--tasks`, and stdout is parseable in the selected format even when a notice is emitted (notices on stderr only).

#### Phase 6: `foreach: command:`
**Model:** opus
- New source, lazy + cached resolution replacing all FOUR `resolve_items` production call sites (help renders `[dynamic]`), resolver cache on `Parser`, loud failure (including at `get_task_names`, replacing the silent `let Ok`), empty-scope stderr notice, `OTTO_FOREACH_COMMAND` recursion guard, mutual-exclusion config error, guards reused (`max_items`, duplicate identifiers, sanitization); amend `docs/foreach-subtasks.md` non-goal with the weighed rationale.
- Tests beyond the criteria: recursion-guard error, mutual-exclusion error, duplicate-line duplicate-identifier error, cwd/env contract.
- **Success criteria:** (a) the YAML example above (with a fixture command) expands and runs; (b) counter test: the command executes exactly once per invocation that needs it, and zero times for `otto --help` (which shows `[dynamic]`) and for targeting an unrelated task; (c) command exiting non-zero fails loudly naming task + command, with nothing on stdout.

#### Phase 6b: Dynamic param `choices`
**Model:** opus
- `choices-command` on `ParamSpec` (serde rename + mutual exclusion in `deserialize_param_map`), explicit Bind/Help builder mode replacing the `Option<&Path>` discriminator, resolution through Phase 6's cache at both bind triggers (direct invocation, propagation validation), `[dynamic choices: <command>]` help rendering, `OTTO_CHOICES_COMMAND` guard, `--tasks` provenance field.
- **Success criteria:** (a) fixture param `choices-command: "printf 'alpha\nbeta\n'"`: `otto switch --svc beta` runs and `--svc nosuch` errors listing alpha, beta; a PROPAGATED invalid value to a dependency's dynamic-choices param errors loudly too; (b) counter test: the command executes zero times for `otto --help`, `otto help switch`, and `otto --tasks` (which carries the `choices-command` provenance field instead), and exactly once per invocation across direct binding plus propagation; help renders `[dynamic choices: ...]`; (c) failure modes all loud, naming task + param + command: non-zero exit, zero output lines, and `choices` + `choices-command` both set (config load error); and a round-trip test proves a `choices-command:` param re-serializes verbatim.
  `Observed on main:` `choices-command` is silently ignored (unknown ParamSpec field): both `--svc beta` and `--svc nosuch` run successfully with no validation.

#### Phase 7: `tty: true`
**Model:** opus
- TaskSpec field, inherit stdout/stderr, skip TaskStreams, `max_parallel` field on `TaskScheduler`, `acquire_many` exclusivity with precondition assert, `--tui` conflict error, marker-line logs.
- **Success criteria:** (a) pty test (`script -qec`) sees `STDOUT: tty` inside a `tty: true` task, and its recorded logs contain the marker line; (b) exclusivity test: no other task overlaps a running tty task; (c) `otto --tui` with a tty task in the run set errors before executing anything.

#### Phase 8: Small items
**Model:** sonnet
- `--no-prefix` flag; positional-args documentation incl. the collision sharp edge.
- **Success criteria:** (a) `otto --no-prefix <task>` emits task output with no `[task]` prefix; (b) `rg -l 'Positional parameters' docs/commands/ README.md` returns at least one file, and that section names the task-name-collision edge.
  `Observed on main:` original criterion said `docs/` and passed vacuously (exit 0) because this design doc itself contains the literal string on this very line: `rg -n 'Positional parameters' docs/` returns only `docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md:253`. **Amended 2026-08-29 (doc defect):** scoped to `docs/commands/ README.md`, which is where the Phase 8 body already says the documentation lands. `rg -l 'Positional parameters' docs/commands/ README.md` on main returns nothing, exit 1.

## Acceptance Criteria

Repro fixtures from the boundary review; every "Observed on main" line was run on 2026-08-28 against installed `otto v1.2.6` at `10e9cac`.

- [ ] Self-reference: fixture `otto: envs: {MYVAR: '$(echo "${MYVAR:-fallback}")'}`; `MYVAR=from-shell otto show` prints `MYVAR=[from-shell]`.
  `Observed on main:` `[show] MYVAR=[fallback]`
- [ ] Serial targeting: fixture `items: [alpha, beta, gamma]`, `parallel: false`; `otto up:gamma` runs exactly gamma.
  `Observed on main:` runs `[up:alpha]`, `[up:beta]`, `[up:gamma]` (control: under `parallel: true`, exactly gamma)
- [ ] Dynamic foreach: fixture `foreach: {command: "printf 'alpha\nbeta\n'", as: svc, parallel: false}`; `otto up` expands and runs `up:alpha`, `up:beta`.
  `Observed on main:` `foreach requires glob, items, or range`, exit 1
- [ ] Machine-readable list: `otto --tasks | jq -e 'type == "object" and (keys | length > 0)'` exits 0 and subtask ids appear in `subtasks` arrays (one map shape in JSON and YAML, per Phase 5).
  `Observed on main:` `error: unexpected argument '--tasks' found`, exit 1
- [ ] Help completeness: `otto --help` lists `-C/--cwd`, `-o/--ottofile`, `--list-subtasks`.
  `Observed on main:` only `-j/--jobs` and `-t/--tui` render
- [ ] Dynamic choices: fixture param `choices-command: "printf 'alpha\nbeta\n'"`; `otto switch --svc nosuch` errors listing alpha, beta, while `otto help switch` executes the command zero times.
  `Observed on main:` `choices-command` silently ignored; `--svc nosuch` runs successfully with no validation

Phase-level criteria observed on main where runnable: nested `$()` fixture warns `Command 'echo "$(basename /a/b' failed with exit code 2` and leaves `NESTED` unset (Phase 3a); tty fixture under `script -qec` prints `STDIN: tty` / `STDOUT: NOT a tty` (Phase 7a). Criteria depending on unshipped surfaces (`--tasks` jq shape, `--no-prefix`, command-source counter test) cannot run on main by definition; their pre-state is the errors recorded above.

## Resolved Decisions

- 2026-08-28: **This doc ships before the two 2026-06-10 docs**; overlapping bullets are de-duplicated in Phase 0. Rationale: small, reproduced, live consumer; the 06-10 docs are unbuilt.
- 2026-08-28: **Serial-group failure skips successors** (preserves today's observable full-run behavior, right call for bring-up, keeps two other docs' test specs valid). Targeting never expands the run set.
- 2026-08-28 (superseded same day, panel round 1): ~~`--tasks` emits JSON only, no format flag~~ -> **`--tasks` is TTY-detected (YAML on a tty, JSON when piped) with one `--format {yaml,json}` override**, which is the full house rule, serves the sed-only consumer via `--format yaml`, and still gives `jq` consumers flag-free JSON when piped.
- 2026-08-28: **`foreach: command:` is lazy + cached**, never runs on `--help`, always runs on enumeration surfaces; failure is loud; empty output is a noticed empty expansion.
- 2026-08-28: **Task params are not available to `foreach: command:`** (params resolve after expansion); stated as a documented limitation, shell env is the dynamic input.
- 2026-08-28: **Nested-prefix suppression is an opt-in `--no-prefix` flag**, no `OTTO_TASK` auto-detection (magic default, loses inner attribution).
- 2026-08-28: **Positional args reframed as documentation** (capability verified working).
- 2026-08-28: **tty task logs carry a marker line**, never silently empty files.
- 2026-08-28 (panel round 1): **`--help` renders `[dynamic]` for command-sourced foreach**, replacing the item-count `resolve_items` call at `parser.rs:1303`; `--help` stays execution-free.
- 2026-08-28 (panel round 1): **the `-j 0` fix stays in remediation** (the hang fires before any tty behavior is reachable); Phase 7 states the max_parallel >= 1 precondition and asserts `acquire_many` requests the initial permit count. The remediation `-C=DIR` bullet does join the Phase 0 de-dup list (Phase 1 fixes it).
- 2026-08-28 (panel round 1): **`OTTO_FOREACH_COMMAND` recursion guard** for command sources; a nested otto resolving the same task errors loudly.
- 2026-08-28 (panel round 1, format amended round 2): **`--tasks` contract frozen**: user tasks only, no builtins, stdout pure data in the selected format (TTY-detect YAML/JSON, one `--format` override), notices/errors on stderr; `jq` consumers pipe for JSON, sed-only consumers use `--format yaml`.
- 2026-08-28 (panel round 1): **no SkipKind refactor in Phase 4**: cascade skips tracked internally by the gate that creates them; up-to-date skips distinguished via `completed_set`; the full provenance refactor stays in the remediation doc.
- 2026-08-28 (panel round 1): **the two chain-edge tests are rewritten** to assert group ordering (disclosed test change).
- 2026-08-28 (panel round 2, conceded): **the serial gate classifies predecessors by the scheduler's existing terminal sets** (`completed_set`/`failed_set`/`skipped_set`), covering the ordinary-`when:`-edge skip case both seats proved would otherwise hang the gate; cascade skips reuse `skipped_set`, no bespoke group-skip set.
- 2026-08-28 (panel round 2, conceded): **DOT/SVG/PNG/PDF graph output changes are disclosed and designed**: serial ordering renders as dashed `order` edges (ASCII unchanged); only the ASCII view collapses subtasks.
- 2026-08-28 (panel round 2): **`--tasks` is one logical shape in both formats**: a map keyed by task name (mirrors the ottofile's `tasks:`); the YAML and JSON encodings never diverge structurally.
- 2026-08-28 (panel round 2, measured): **the dep-fails serial fixture exits non-zero and stays that way after Phase 4** (author's run: exit 1; the panel's exit-0 claim did not reproduce); the fixture is pinned in Phase 4 criterion (c) with visible skip reasons for cascade-skipped members.
- 2026-08-28 (Scott, post-panel): **dynamic param `choices` pulled into scope as Phase 6b**, reversing the initial parking. Help decision: bind path executes (once, cached), help paths render `[dynamic choices: <command>]` and never execute, one rule shared with foreach. Empty output is a loud error (an empty valid set is a misconfiguration), unlike foreach's legitimate empty scope. `--tasks` reports `choices-command` provenance without executing.

## Alternatives Considered

### Ordering-only edge type for serial foreach
- **Description:** Keep chain edges but tag them ordering-only; skip them in `collect_transitive_deps`.
- **Pros:** Smaller parser diff; reuses edge machinery.
- **Cons:** `task_deps` copy at `parser.rs:799` is unfiltered; scheduler dep check hard-errors on never-scheduled deps; every edge consumer needs the tag check, forever.
- **Why not chosen:** A group property has exactly one consumer (the ready loop); edges have many.

### Run successors after a serial predecessor fails (pure ordering)
- **Description:** Ordering constrains start time only; gamma runs after alpha fails.
- **Pros:** Consistent with `parallel: true` sibling independence.
- **Cons:** Wrong for the actual use case (stack bring-up on a half-up stack); silently changes full-run behavior; invalidates the remediation Phase 2 and architecture-doc timeout test specs.
- **Why not chosen:** Fail-closed wins; skip with a visible reason.

### `--tasks` as JSON-only or with a boolean `--json` flag
- **Description:** Emit JSON unconditionally (this doc's own first draft), or mirror History/Stats' boolean `--json`.
- **Pros:** JSON-only is the least code; `--json` matches two existing siblings.
- **Cons:** JSON-only quotes half the house rule and ships the one format the demonstrated consumer (sed-only doctor.sh, zero jq in otto-dev/scripts, panel-verified) cannot read; boolean format flags are banned outright, and History/Stats' `--json` is the legacy to converge away from, not precedent.
- **Why not chosen:** TTY-detect + one `--format` override is the full house rule and serves both consumer shapes with no extra surface.

### Auto-suppress the nested prefix via `OTTO_TASK`
- **Description:** Inner otto detects it's running inside a task and drops its own prefixes.
- **Pros:** Zero consumer changes.
- **Cons:** Silent fleet-wide output change; inner parallel tasks lose attribution with no opt-out lever visible at the call site.
- **Why not chosen:** Defaults are opt-in; magic that changes output shape gets ripped out.

### Execute `choices-command` for help rendering
- **Description:** Run the command when help renders so `[possible values: ...]` shows live values.
- **Pros:** Help shows the real valid set.
- **Cons:** Makes `otto help <task>` execute ottofile code, breaking the one rule Phase 6 establishes (dynamic sources never execute for help); values can be stale a second later anyway.
- **Why not chosen:** One posture for both dynamic features; the marker names the command so a human can run it themselves.

### Eager resolution for `foreach: command:`
- **Description:** Run the command wherever `resolve_items` runs today (every invocation).
- **Pros:** No caching logic; identical to glob/items flow.
- **Cons:** `otto --help` and every unrelated invocation execute a user command; directly worsens the architecture doc's documented liability.
- **Why not chosen:** Lazy + cached is barely more code and strictly narrower execution.

## Technical Considerations

### Dependencies
Zero new crates: regex 1.12.2, serde_json 1.0, tokio (semaphore) are direct deps already.

### Performance
- Lazy foreach-command resolution makes some invocations cheaper than eager would be; at most one subprocess per command-sourced foreach per invocation.
- `acquire_many` for tty serializes only runs containing tty tasks.
- Everything else is load-time logic on already-loaded data.

### Security
- `foreach: command:` is a new load-adjacent execution site in a YAML format that already executes `$()` at load. Posture: lazy (never on `--help`), documented in the feature docs, and flagged to the architecture doc which owns narrowing the overall posture.
- No secrets handling; command runs with the invoking user's environment like every other otto subprocess.

### Testing Strategy
- Every phase carries its regression tests (named in the success criteria); property test for env ordering; interleave test for serial ordering; pty test via `script -qec` for tty; sentinel/counter tests for no-execution and exactly-once claims.
- Tests demonstrated to bite: each phase breaks its code once to show the new test fails (per house rule).
- `otto ci` gates every phase.

### Rollout Plan
Per-phase commits on main via the standard flow (`otto ci` -> commit), one commit per phase; `bump` after the tranche or per Scott's call. All changes additive; the only observable behavior changes are the four bug fixes themselves. No migration, no schema change, no config break: existing ottofiles load unchanged.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Serial-group gate deadlocks with mixed edges (foreach subtask also has explicit `before:`) | Low | High | Gate composes with (never replaces) dep readiness; integration test with a dependent serial group |
| Env per-expression context regresses cross-ref resolution order | Med | Med | Property test over randomized key insertion orders; deferral path untouched |
| Quote-aware scanner mismatches sh's own parse of an edge case | Low | Med | Scanner scope limited to finding the boundary; sh still executes the content; adversarial test table |
| `foreach: command:` resolution needed during arg partitioning slows `otto <task>` | Low | Low | Resolution only when the parent or a subtask-shaped token is requested; cached |
| tty task wedges the run (interactive prompt never answered in CI) | Med | Low | Documented: tty tasks are for interactive use; CI ottofiles shouldn't carry them; a future per-task timeout (architecture doc Phase 5) is the structural guard |
| Help snapshot test churns on unrelated flag additions | Med | Low | That churn is the feature: drift becomes a visible diff |

## Open Questions

(none)

## References

- Boundary review (spec source): <internal review link, not reproduced here> (local: /tmp/claude/otto-boundary-review/index.md)
- Handoff artifact: /tmp/claude-1000/handoff-otto-fixes.md
- `docs/design/2026-06-10-code-review-remediation.md` (In Review; Phase 0 amends)
- `docs/design/2026-06-10-architecture-product-completion.md` (In Review; Phase 0 amends)
- `docs/foreach-subtasks.md` (non-goal amended in Phase 6)
- `docs/design/2026-03-26-directory-flag.md` (house template; the doc whose help claim drifted)
- Reference SHAs: `otto@10e9cac`, `otto-dev@9724298`; installed `otto v1.2.6`

## Addendum: Parked Items

- **Dynamic param `choices`** was parked here through panel review, then pulled into scope by Scott on 2026-08-28 (now Phase 6b). Recorded so the reversal is visible.
- **`OTTO_TASK`-based prefix auto-detection.** Rejected (see Alternatives); recorded so it isn't re-proposed.
- **Whole-run/per-task timeouts, `--plan`, lazy env evaluation.** Architecture doc scope; not re-litigated here.
