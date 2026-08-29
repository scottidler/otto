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

Use [setup-otto](https://github.com/scottidler/setup-otto) to install otto in your workflows:

```yaml
- uses: scottidler/setup-otto@v1

- name: Run otto tasks
  run: otto ci
```

See [setup-otto](https://github.com/scottidler/setup-otto) for full documentation and options.

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
otto sw philo
# svc=philo
```

### Positional parameters and the task-name collision edge

otto splits a multi-task command line by scanning for tokens that match a
declared task name, before params are bound. A positional value that happens
to equal another task's name is misread as the start of that task instead of
being bound to the preceding task's param (`otto sw philo` above binds
`svc=philo` only because no other task in that ottofile is named `philo`).
Avoid naming a task the same as a value you expect to pass positionally to
another task. See [`docs/commands/positional-parameters.md`](docs/commands/positional-parameters.md)
for the full declaration shape and a reproduced example of the collision.

## Version Reporting

The `otto` binary supports `--version` and `-v` flags:

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
4. **Push** the tag: `git push --tags`.
5. **Build** the binary. The version will be embedded from the tag and `git describe`.
6. **Create a GitHub Release** and upload the binary. The version in the binary will match the release tag.

> If the version in `Cargo.toml` does not match the latest tag, a warning will be printed at build time.
