# Positional parameters

A task param declared with no short/long flag form is bound positionally: its
value is whatever bare word follows the task name on the command line. This
already works end to end (`ParamType::POS` in `src/cfg/param.rs`, wired into
clap's positional args in `src/cli/parser.rs`); this page documents the
declaration shape and one sharp edge, it does not change any behavior.

## Declaring a positional parameter

Give the param a bare name (no `-x|--flag` form) and it is divined as
positional:

```yaml
tasks:
  sw:
    params:
      svc:
        help: service name
    bash: |
      echo "svc=${svc}"
```

```bash
otto sw philo
# svc=philo
```

Compare a flag-style param (`-s|--svc: ...`), which requires `otto sw --svc
philo` or `otto sw -s philo` instead of a bare positional value. Use a
positional param when the value is the task's one obvious argument (a service
name, a file path); use a flag when the task has several optional values or
the call site benefits from a name at the call site.

## The sharp edge: positional values that collide with a task name

otto splits a multi-task command line into per-task argument groups by
scanning for tokens that match a declared task name (`partitions()` in
`src/cli/parser.rs`). That scan does not know a positional value is coming;
it only knows task names. If a positional value happens to equal another
task's name, the value is misread as the start of a new task invocation
instead of being bound to the preceding task's param.

Reproduced on a real fixture:

```yaml
tasks:
  sw:
    params:
      svc:
        help: service name (positional)
    bash: |
      echo "svc=${svc}"
  philo:
    bash: |
      echo "philo task ran"
```

```bash
$ otto sw philo
[sw] script.sh: line 17: svc: unbound variable
[philo] philo task ran
[sw] failed
[philo] finished successfully
```

`otto sw philo` did not bind `svc=philo`. It split into two invocations,
`sw` (positional left unset, task fails) and `philo` (ran as its own task).
A non-colliding value works exactly as declared:

```bash
$ otto sw someservice
[sw] svc=someservice
```

**Avoid this by not naming a task the same as a value you expect to pass
positionally to another task.** There is no otto-side disambiguation today;
the fix, if one is ever needed, lives in `partitions()`'s task-name scan, not
in the param declaration.

## See also

- [Flag support](../flag-support.md) - the full `ParamType` (`FLG`/`OPT`/`POS`)
  detection rules and flag-style param examples.
- [`otto --tasks`](tasks.md) - each param's `positional` field in the
  machine-readable task list.
