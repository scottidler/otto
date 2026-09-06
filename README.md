# otto
otto program for make-like task mgmt via yaml file

## Installation

### Quick Install

```bash
curl -fsSL https://raw.githubusercontent.com/otto-rs/otto/main/install.sh | bash
```

Options:
```bash
# Install to a custom directory
curl -fsSL https://raw.githubusercontent.com/otto-rs/otto/main/install.sh | bash -s -- --to ~/bin

# Install a specific version
curl -fsSL https://raw.githubusercontent.com/otto-rs/otto/main/install.sh | bash -s -- --version v1.0.0
```

### GitHub Actions (Recommended for CI/CD)

Use [setup-otto](https://github.com/otto-rs/setup-otto) to install otto in your workflows:

```yaml
- uses: otto-rs/setup-otto@v1

- name: Run otto tasks
  run: otto ci
```

See [setup-otto](https://github.com/otto-rs/setup-otto) for full documentation and options.

### From Source

```bash
cargo install --git https://github.com/otto-rs/otto
```

## Task Parameters

A task param declared with no short/long flag form binds positionally: the
bare word following the task name on the command line becomes its value.

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
otto sw web
# svc=web
```

### Positional parameters and the task-name collision edge

otto splits a multi-task command line by scanning for tokens that match a
declared task name, before params are bound. A positional value that happens
to equal another task's name is misread as the start of that task instead of
being bound to the preceding task's param (`otto sw web` above binds
`svc=web` only because no other task in that ottofile is named `web`).
Avoid naming a task the same as a value you expect to pass positionally to
another task. See [`docs/commands/positional-parameters.md`](docs/commands/positional-parameters.md)
for the full declaration shape and a reproduced example of the collision.

## Environment Variables

Global environment variables live under `otto.envs:`; task-scoped ones live
under the task's own `envs:` and are layered on top. **There is no root-level
`envs:` key** — see [`docs/commands/ottofile-reference.md`](docs/commands/ottofile-reference.md)
for the full key reference, and
[`docs/commands/ottofile-strict-schema-migration.md`](docs/commands/ottofile-strict-schema-migration.md)
for why that distinction matters.

```yaml
otto:
  envs:
    PROJECT_NAME: myproj

tasks:
  build:
    envs:
      BUILD_DIR: build
    bash: |
      mkdir -p "$BUILD_DIR"
      echo "Building $PROJECT_NAME in $BUILD_DIR"
```

```bash
otto build
# Building myproj in build
```

## Usage

`otto` runs tasks named in an ottofile (`otto.yml`, `.otto.yml`, `otto.yaml`,
`.otto.yaml`, `Ottofile`, or `OTTOFILE`), discovered by walking up from the
current directory unless `-o/--ottofile` or `-C/--cwd` says otherwise.

```bash
otto <task> [<task> ...]   # run one or more tasks by name
otto --help                # list every task plus the six builtins below
otto -j 4 build            # cap concurrency at 4 (default: number of CPUs,
                            # or otto.jobs in the ottofile if set)
otto -t build              # run under the interactive TUI dashboard
otto --no-prefix build     # drop the "[task]" prefix from task output
```

**Builtins** (capitalized, run like any other task):

| Builtin | What it does |
|---|---|
| `Clean` | Remove old runs from `~/.otto/` (see `docs/commands/clean.md`) |
| `Convert` | Convert a Makefile (via stdin) to an ottofile: `cat Makefile \| otto Convert > otto.yml` |
| `Graph` | Visualize the task dependency graph |
| `History` | View execution history (see `docs/commands/history.md`) |
| `Stats` | View execution statistics (see `docs/commands/stats.md`) |
| `Upgrade` | Upgrade otto to a newer release (see `docs/commands/upgrade.md`) |

Every task and builtin has its own `--help`, e.g. `otto Convert --help` or
`otto build --help`. The full ottofile key reference lives at
[`docs/commands/ottofile-reference.md`](docs/commands/ottofile-reference.md).

## Version Reporting

The `otto` binary supports `--version` and `-V`:

```
$ otto --version
otto v0.1.0-3-gabcdef
```

- The version is driven by the latest annotated git tag and the output of `git describe`.
- If the current commit is exactly at a tag (e.g., `v0.1.0`), the version will be `otto v0.1.0`.
- If there are additional commits, it will show something like `otto v0.1.0-3-gabcdef`.

## Release & Versioning Process

1. **Bump the version in `Cargo.toml`** to the new release version (e.g., `0.2.0`).
2. **Commit** the change.
3. **Tag** the commit with an annotated tag: `git tag -a v0.2.0 -m "Release v0.2.0"`.
4. **Push the tag**: `git push --tags`. `.github/workflows/release-and-publish.yml` triggers on the tag push, builds Linux and macOS binaries, and creates the GitHub Release with them attached. The version embedded in the binary comes from the tag via `git describe`.
