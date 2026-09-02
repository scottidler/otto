# `foreach.buffer`: ordered output for a parallel foreach

`tasks.<name>.foreach.buffer: true` (design doc
`docs/design/2026-08-31-buffered-foreach-computed-envs-required-params.md`,
Phases 3-4) makes a `parallel: true` foreach print each subtask's output as
one contiguous block, in item order, instead of interleaving lines from
whichever subtasks happen to be writing at the same moment.

```yaml
tasks:
  status:
    foreach:
      command: "scripts/stack.sh scope status"
      as: svc
      parallel: true
      buffer: true
    bash: |
      scripts/svc.sh run "${svc}" status
```

## What it guarantees

- **Item order, not completion order.** Blocks print in the order the
  subtasks were declared/expanded (the same order as each subtask's
  `OTTO_FOREACH_INDEX`), never in the order they finish. A slow first item
  holds up display of a fast second item; execution itself stays fully
  concurrent the whole time, only the terminal write is serialized.
- **No interleaving.** No line from one subtask's block can appear between
  two lines of another's, and no scheduler-emitted status/skip/failure line
  can land inside a block either — all seven places that write to the
  terminal take the same output lock, and replay holds it for one whole
  block.
- **The subtask's status line travels with its block**, appended at replay
  time rather than printed when the subtask actually finishes. Otherwise
  status lines would arrive in completion order while the blocks above them
  arrive in item order.
- **A skipped or failed subtask still occupies its slot** and still advances
  the cursor; a failed item's block still prints and the parent foreach
  still fails.

## Replay order within a block: stdout, then stderr

Buffering replays a subtask's two capture files, `stdout.log` then
`stderr.log`, in that fixed order. This is documented, not promised as true
arrival order: the two streams are drained by two independent tokio tasks
with nothing synchronizing them against each other, so "what actually
interleaved at the process level" is already not recoverable, buffered or
not. No new capture file is added; buffering reads the two logs every task
already writes and adds a replay policy on top, not a new run-dir artifact.

## Truncation is loud, never silent

Reaching a terminal state is not the same as having complete output: a
subtask's output-drain can finish with a processing error, a join error, or
time out (`OUTPUT_PROCESSING_TIMEOUT_SECS`) while the process itself still
exits 0. When that happens the block ends with a marker naming the
condition and the log path, e.g.:

```
otto: WARNING: say:alpha stdout output may be truncated: output processing timed out; full log: <run>/tasks/say:alpha/stdout.log
```

instead of a short block followed by a "finished successfully" line that
would otherwise read as if nothing were missing.

### Cancellation (SIGINT mid-group)

A buffered group interrupted mid-run still flushes in item order, one thing
per item, and never stops at the first non-terminal one:

| Item state at cancellation | What replay prints |
|---|---|
| Already terminal (completed/failed/skipped) | its block, as in a normal run |
| Report sent but not yet consumed by the scheduler | its block (the logs are complete) |
| Child launched, then killed by the cancellation | `otto: <name> was killed mid-run; its logs are partial and are not replayed: <stdout.log> <stderr.log>` |
| Body spawned, child never launched / queued / blocked | `otto: <name> did not start` |

Killed-child logs are never replayed as a partial block; run-dir paths are
printed instead so nothing is silently discarded and nothing looks complete
when it is not.

## Where `buffer` does nothing

- **`tty: true` + `buffer: true` on the same task is a load error.** A `tty`
  task owns the terminal exclusively, so there is nothing left to buffer.
  `tty: true` on a foreach task WITHOUT `buffer` is unaffected and keeps
  printing today's unprefixed, contiguous blocks (a tty task already runs
  exclusively, so its subtasks never overlap).
- **`buffer: true` with `parallel: false` is accepted and inert.** Subtasks
  already run one at a time, so replay reproduces the same output either
  way; nothing to buffer against.
- **`buffer: true` under `--tui` is accepted and inert.** The TUI already
  suppresses the terminal leg and gives every task its own pane; buffering
  has nothing to add there and the replay cursor is empty in TUI mode.
- **`--no-prefix` behaves the same as everywhere else**: prefixes are still
  stripped per line, both inside and outside a buffered block. Buffering
  only fixes contiguity, not prefix attribution.

## `foreach.jobs`: per-group concurrency, overriding `-j`/`otto.jobs`

`tasks.<name>.foreach.jobs: all | <N>` (design doc
`docs/design/2026-09-01-cancellation-reaping-and-foreach-concurrency.md`,
Phase 2-3) overrides the global concurrency cap for one foreach group's
items only. It exists for a group of tasks that never exit on their own (a
log tail, a watcher, a dev server): under the global `-j`/`otto.jobs` cap,
items past the cap never get a permit and never start, silently.

```yaml
tasks:
  logs:
    foreach:
      command: "scripts/stack.sh scope logs"
      as: svc
      parallel: true
      jobs: all          # every item gets its own permit, ignoring -j
```

- **`jobs: all`** gives every item in the group its own permit, so all of
  them start regardless of the global cap.
- **`jobs: <N>`** (a positive integer) caps the group at `N` concurrent
  items, independent of `-j`/`otto.jobs`.
- **Requires `parallel: true`.** `jobs` with `parallel: false` is a load
  error: serial already means one item at a time, so a concurrency override
  is incoherent (`Task '<name>': foreach.jobs cannot be combined with
  parallel: false; serial means one item at a time, so a concurrency
  override is incoherent`). Running the same task with `--Serial` is
  rejected for the same reason, when the run is set up rather than when the
  file loads: `Task '<name>': --Serial cannot be combined with foreach.jobs`.
- **`jobs: 0` is a load error**, not "unbounded": `foreach jobs: 0 is not a
  valid count; write \`jobs: all\` to run every item at once`.
- **`jobs: all` with `buffer: true` is legal and expected.** Buffering is a
  display policy (how output is replayed); `jobs` is a scheduling policy
  (how many permits the group gets). The two do not interact.

### `tty: true` on the same task

A foreach task's `tty: true` and `foreach.jobs` ask for opposite things:
exclusive ownership of the terminal versus one permit per item shared out
across the group. Setting both on one task is a load error (`Task '<name>':
foreach.jobs cannot be combined with tty; a tty task owns the terminal
exclusively, so it runs exclusively and a per-group concurrency override
cannot be honored`), the same rejection and the same reason `foreach.buffer`
with `tty` already gets. The message names the task you wrote, not one of
its expanded items.

`tty: true` on a foreach task *without* `jobs` is unaffected. So is a
`tty: true` task elsewhere in the run: it and a `jobs:` group are kept off
each other's terminal by the scheduler, which is the next section.

### The accepted consequence: a `jobs: all` group can hold up a later `tty: true` task

A group with `jobs: all` whose items never exit holds up a later `tty: true`
task for as long as the group runs, which for a log tail is forever. otto
never puts two writers on one terminal, so a `tty: true` task waits until
every exempt item has finished. **This is not a regression.** A single
never-exiting task holds a shared permit forever today and starves a later
`tty` task identically; `jobs: all` does not introduce the hazard, it just
makes it possible to hit on purpose. It is the accepted cost of asking for
the exemption, which is why a group like `logs` belongs last in a task list
by construction.

**Ordinary tasks are not held up.** A later task without `tty: true` starts
beside the group, even at `-j 1`: the group's items are exempt from the
global cap, so they do not occupy it. Only a `tty: true` task waits.

**A waiting `tty: true` task can be skipped past indefinitely.** Admission is
not first-come-first-served. A `tty: true` task the scheduler cannot admit
yet goes back to the head of the ready queue and the pass keeps going, so
exempt items that become ready afterwards start ahead of it. Against a group
whose items keep arriving, the tty task waits indefinitely. That is
deliberate: stopping the pass at the first task it cannot admit would mean
one waiting `tty: true` task keeps a `jobs: all` group from ever starting,
which is the case the key exists for.

## Non-goals

- Emitting blocks in completion order instead of item order: excluded by
  design (see the design doc's Non-Goals).
- Buffering a non-foreach task's output: a single task's output is already
  contiguous, so there is nothing to group.
- A merged `output.log` interleaving stdout and stderr in true arrival
  order: excluded, since no such ordering guarantee exists in otto today for
  either stream to promise.
