# Prepare worker environments

Once a benchmark works, you may want to record the software it uses so you can
check it before a later run. A locked environment records that software and lets
Metewand verify it before starting workers.

These recipes use uv for a Python project or Nix for a benchmark that already
provides a flake. Locking the software does not make timings equal across
machines. You still need to consider the hardware and conditions of each run.

## Lock Python dependencies with uv {#lock-uv-worker-environments}

Install uv and a compatible Python interpreter. Your project directory should
contain `pyproject.toml` and `uv.lock`.

Add this environment to the manifest:

```yaml
environments:
  python:
    kind: uv
    project: .
    lockfile: uv.lock
```

Set the generator, implementation, and evaluator workers' `environment` to
`python`. With function declarations, add `environment: python` beside each
`function` field.

If you use Git, stage new source files before locking so Metewand includes them.
Then prepare and check the environment:

```sh
uv lock
metewand lock
metewand lock --check
metewand run --locked --output-dir ../locked-run
```

Metewand prepares a separate environment and leaves your project's `.venv`
unchanged. Commit `uv.lock` and `metewand.lock` with the benchmark. Keep the run
outside the project sources, as `../locked-run` does here.

A locked run checks the prepared software before starting. If it has changed or
is missing, run `metewand lock` again to prepare it. After changing
dependencies, update `uv.lock` with uv before preparing the Metewand environment
again.

The [locked uv
example](https://github.com/jolars/metewand/tree/main/examples/locked-uv) shows
a complete project, including how to select dependency groups.

## Lock worker programs with Nix {#lock-nix-worker-environments}

Use this recipe when a Nix flake already provides your worker programs. For a
Python package output named `python`, declare:

```yaml
environments:
  python:
    kind: nix
    flake: .
    installable: '.#python'
    system: x86_64-linux
    output-kind: package
    output: out
```

The flake must expose `packages.x86_64-linux.python`, with a program such as
`bin/python3` in its selected output. Set each worker's `environment` to
`python`, then use the same `metewand lock`, `lock --check`, and `run --locked`
commands shown above.

Prepare the flake's native lockfile first. Metewand does not update it for you.
Commit `metewand.lock` with the benchmark, and stage new source files before
locking. See the [locked Nix
example](https://github.com/jolars/metewand/tree/main/examples/locked-nix) for a
pinned flake and runnable comparison.

Return to [running the comparison](running.md) when the environment is ready.
