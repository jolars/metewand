# Prepare worker environments

Workers need the interpreter and packages used by your benchmark. Local
environments are useful while developing. To record and verify the software used
for a run, prepare a locked environment with uv or Nix.

Local workers run trusted code. CPU, thread, and memory requests are best
effort; the local backend cannot enforce them or isolate network access. Locked
workers retain their backend's existing execution controls.

## Declare worker variables

Metewand controls the environment variables given to local workers. Declare
numerical-library settings in the manifest:

```yaml
environments:
  local:
    kind: local
    environment-variables:
      OMP_NUM_THREADS: "1"
      OPENBLAS_NUM_THREADS: "1"
      MKL_NUM_THREADS: "1"
      PYTHONHASHSEED: "0"
```

Set the relevant workers' `environment` to `local`. Metewand includes these
literal strings in worker identities and provenance. You cannot override
variables that Metewand owns, such as private home and cache paths or protocol
channels. Library settings do not pin workers to CPUs or enforce resource
limits.

## Lock Python dependencies with uv {#lock-uv-worker-environments}

Install uv and a compatible Python interpreter. Your project or workspace root
must contain `pyproject.toml` and an author-owned `uv.lock`:

```yaml
environments:
  python:
    kind: uv
    project: .
    lockfile: uv.lock
    groups: [benchmark]
    extras: []
```

Set the generator, implementation, and evaluator workers' `environment` to
`python`. Function workers choose their adapter automatically. Explicit Python
binding workers also need `protocol-transport: socket`. Other workers can use
`runner: python` or a program such as `python3`; installed console entry points
are also available. Metewand resolves programs within the prepared environment
and preserves its virtual-environment context.

Prepare and check the environment before running:

```sh
uv lock
metewand lock
metewand lock --check
metewand run --locked --output-dir ../results
```

Project requirements and `.python-version` choose an installed interpreter.
Metewand disables automatic Python downloads. `groups` and `extras` are
optional; when omitted, only base dependencies are installed and uv's default
groups are disabled. Metewand sorts and deduplicates these selections before
hashing them. Checking and planning do not invoke uv or import benchmark code.

`metewand lock` prepares a separate environment using `--locked`,
`--no-editable`, and copied package files. It leaves your `uv.lock` and `.venv`
unchanged. It captures project configuration and excludes ambient uv
configuration and Python search paths. Local dependencies and workspace members
must stay inside the captured repository sources. Stage new source files before
locking.

For a uv-only benchmark, `lock --check` and `run --locked` work offline without
uv or Nix installed. They check source content, the interpreter and standard
library, installed files and symlink targets, the platform, and worker launches.
Missing, changed, or added installed files cause the check to fail before a run
directory is created. Checking never fetches, builds, syncs, or repairs files.
Run `metewand lock` again to prepare missing state or rebuild changed sources,
even when package metadata has not changed.

### Keep the prepared environment available

Metewand stores prepared environments in your user data directory:

- Linux: `$XDG_DATA_HOME/metewand`, or `~/.local/share/metewand`.
- macOS: `~/Library/Application Support/metewand`.
- Windows: `%LOCALAPPDATA%/metewand`.

To choose another root, set `METEWAND_DATA_DIR` to an absolute path outside the
captured sources. Use the same root for locking and running. Prepared
environments keep their installation paths and cannot be copied as portable
virtual environments. Metewand reuses verified environments and never overwrites
an older one.

For identities, Metewand normalizes generated metadata and launcher paths;
different built package content still produces different resolved evidence.
Verification checks exact installed bytes separately. Workers use a private
bytecode cache, so changes to the host's bytecode cache do not invalidate the
locked environment.

The lock covers the prepared Python runtime and packages. Native extensions are
verified as files, and preparation records uv and build settings. This does not
capture every external native library or establish a reproducible build
toolchain. See the [locked uv
example](https://github.com/jolars/metewand/tree/main/examples/locked-uv) for a
complete comparison.

## Lock worker programs with Nix {#lock-nix-worker-environments}

Declare a Nix environment for every worker in the plan:

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

Set each worker's `environment` to `python`. The flake must expose
`packages.x86_64-linux.python`, whose selected output provides the worker
program, such as `bin/python3`. Package programs resolve within that output.

For apps, use `output-kind: app`, an app name as `output`, and the matching
installable. Workers must name the app's executable. Repository scripts use
their locked shebang interpreter. Native executables must come from a realized
Nix output.

```sh
metewand lock
metewand lock --check
metewand run --locked --output-dir ../results
```

Locking may fetch sources and build outputs. Complete native flake locks first;
Metewand does not update them. Commit `metewand.lock` with the benchmark. Local
flake snapshots exclude Git metadata, so flakes cannot depend on `self.rev`.
Source links, special files, and Git submodules are unsupported.

`lock --check` and `run --locked` verify sources, launch settings, executable
hashes, and the full runtime closure, including its contents and references.
Missing or changed paths cause a failure. Run `metewand lock` again to prepare
missing outputs or refresh changed source evidence. Use Nix's own tools to
update native dependencies. Locked runs hold temporary GC roots while running;
the lockfile does not permanently protect outputs from Nix garbage collection.

Locked workers still run trusted local processes with their existing network
access. They do not provide archival isolation. `renv` environments are not yet
supported by the lock command. The [locked Nix
example](https://github.com/jolars/metewand/tree/main/examples/locked-nix)
provides a pinned flake and runnable comparison.

## Include the files you want to lock

For Nix and uv, Git snapshots include tracked working-tree files, including
uncommitted edits. Stage new files before locking. Outside Git, snapshots
include the whole repository tree. Keep run and cache directories outside
captured sources. Generated `.venv` directories for declared uv projects are
excluded. The snapshots exclude `metewand.lock`; worker source bundles must
exclude it too.

A source pattern such as `benchmark/*.py` permits `benchmark/outputs/run`,
because the run directory and its contents cannot match the pattern. A matched
directory includes its whole tree: `benchmark/output.py/run` would overlap that
same pattern. With `benchmark/**/*.py` or the whole benchmark directory as
sources, choose a run directory outside that selection. The same rules apply to
an explicit artifact cache.

In a mixed Nix and uv benchmark, each backend checks its own environment. Older
version-1 Nix locks remain readable and retain their original identities. See
[the reproducibility model](../../design.md#reproducibility-model) for what each
backend records, and [pinned images](extending.md#run-workers-in-a-pinned-image)
for container environments.

Return to [running the comparison](running.md) once the environments are ready.
