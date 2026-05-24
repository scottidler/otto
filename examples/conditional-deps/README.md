# Conditional task dependencies

Otto's task-to-task edges carry an optional `when:` condition that controls
when a dependent task runs relative to its source's outcome.

## The three `when:` values

| `when:`   | Dependent runs when source has reached this terminal state |
|-----------|-------------------------------------------------------------|
| `success` | Source `Completed` (default; bare strings mean `when: success`) |
| `failure` | Source `Failed`                                              |
| `always`  | Source reached any terminal state (`Completed`, `Failed`, or `Skipped` source is `Unreachable`) |

A source that ended `Skipped` makes every dependent edge to it `Unreachable`,
so skips cascade through the graph rather than silently treating the source as
"made."

## Three patterns

### 1. Fixer (via `on-failure:` sugar)

```yaml
fmt-check:
  bash: cargo fmt --all --check
  on-failure:
    - fmt-fix

fmt-fix:
  bash: cargo fmt --all
```

`on-failure: [fmt-fix]` is parse-time sugar that desugars to:

```yaml
fmt-check:
  after:
    - task: fmt-fix
      when: failure
```

Both forms are equivalent. The sugar keeps the locality on the host task
(the one declaring the failure relationship).

### 2. Fixer (long form)

```yaml
lint-check:
  bash: cargo clippy --all -- -D warnings
  after:
    - task: lint-report
      when: failure

lint-report:
  bash: cargo clippy --all 2>&1 | head -100
```

Use the long form when you prefer to write the relationship inverted, or
when you need a `when:` value other than `failure` (for example, `always`
for cleanup).

### 3. Cleanup (`when: always`)

```yaml
test:
  envs:
    TEST_TMPDIR: "$(mktemp -d)"
  bash: cargo test --all-features

cleanup-tmpdir:
  after:
    - task: test
      when: always
  bash: |
    set +e
    rm -rf "$TEST_TMPDIR"
    exit 0
```

`cleanup-tmpdir` runs after `test` regardless of test's outcome.

**Footgun:** a `when: always` cleanup task can itself fail. The pattern above
uses `set +e` and an explicit `exit 0` so cleanup failure doesn't pollute the
run's exit code beyond the original test failure.

## Behavior change: drain-on-failure

When a task fails, otto no longer aborts the whole run immediately. It
records the failure, lets in-flight tasks finish, and queues any
`when: failure` or `when: always` dependents that become reachable. The
first failure is the run's exit error after the drain completes. Tasks
whose edges become `Unreachable` are marked `Skipped`.

## Foreach aggregation

For foreach tasks, the virtual parent is a real aggregator task. Its
status is derived from its subtasks:

- any subtask `Failed` -> parent `Failed`
- else any subtask `Skipped` -> parent `Skipped`
- else (all `Completed`) -> parent `Completed`

A downstream `{task: <foreach-parent>, when: failure}` therefore fires when
any subtask of the parent failed. A `when: failure` dependent does *not*
fire if subtasks were `Skipped` (e.g., because a prerequisite of the foreach
group failed) - the parent aggregated to `Skipped`, not `Failed`.
