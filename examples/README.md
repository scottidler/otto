# otto examples

One line per example. Run any of them with `otto -o examples/<name>`
(three lack a discoverable ottofile name and need `-o examples/<name>/<file>`
explicitly - noted below).

- **basic-dependencies** - three plain tasks, one running `before:` another
- **build-pipeline** - `compile`/`link`/`build` chain that reads and writes real files (`src.c`)
- **build-test-deploy** - a `build`/`test`/`deploy`/`cleanup` chain plus standalone `lint`/`format`/`docs`/`notify` tasks
- **complex-workflow** - a wider dependency graph with concurrent branches
- **conditional-deps** - the `on-failure:` sugar, fixing formatting when a check fails
- **data-flow-bash** - `envs:` and variable interpolation across tasks
- **data-passing-demo** - `otto_set_output`/`otto_get_input` chained bash -> bash -> python -> bash
- **dependency-ordering** - `before:`/`after:` ordering with three tasks (`one`, `two`, `three`)
- **diamond-dependencies** - a diamond-shaped dependency graph (fan-out then fan-in)
- **environment-variables** - global (`otto.envs`) vs task-scoped (`tasks.<name>.envs`) environment variables
- **ex2** - `-o`/positional param binding on a single task (`.otto.yml`, not `otto.yml`)
- **file-dependencies** - `input:`/`output:` file dependencies, including glob patterns
- **flags** - every `ParamType` (`FLG`/`OPT`/`POS`) in one task; ottofile is `flag_demo.yml`, run with `otto -o examples/flags/flag_demo.yml demo`
- **foreach-buffer** - `foreach.buffer: true` and `otto.envs-command`
- **foreach-glob** - `foreach: {glob: ...}` subtasks generated from matching files
- **foreach-items** - `foreach: {items: [...]}` subtasks from an explicit list
- **foreach-range** - `foreach: {range: ...}` subtasks over a numeric range
- **hello-world** - the smallest possible ottofile
- **interactive-demo** - `tty: true` tasks that read the real terminal (a shell, `read`, vim, a Python REPL)
- **parallel-tasks** - independent tasks that run concurrently, joined by a `final` task
- **subtask-targeting** - running one foreach subtask by name vs. the whole parent
- **tui-demo** - `-t`/`--tui` dashboard mode against a mix of fast and slow tasks
