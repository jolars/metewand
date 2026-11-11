# Run and analyze the comparison

Use `metewand plan` after each manifest change to review the experiment summary.
Start with a small run before increasing the grid or repetition counts.

The default output shows selected experiments, their problem, dataset and
implementation names, expanded counts, concurrency, and timing settings. It
summarizes long name lists and omits individual identities and slots. To inspect
the full expansion or consume it in a script, use:

```sh
metewand plan --verbose
metewand plan --output json > plan.json
metewand plan --output jsonl > plan.jsonl
```

`--verbose` expands human output. JSON and JSONL always contain the complete
versioned plan, regardless of `--verbose`.

## Select a named experiment

Keep full, smoke, and example experiments in one manifest. Set
`default-experiment: full` at the top level to choose the usual run. Without
that setting, commands select every experiment as before.

```sh
metewand plan --experiment smoke
metewand check --experiment smoke --workers
metewand run --experiment smoke --output-dir outputs/smoke
```

Repeat `--experiment` to select a union in manifest order. Explicit selection
overrides the default; `--select` narrows the resulting plan. Selection belongs
to the invocation, so repeat it when resuming. Static checks validate the entire
manifest. Worker checks launch only workers needed by the selected experiments.

## Declare worker variables

Local workers receive an isolated environment. Declare numerical-library
settings explicitly rather than relying on the shell that launches Metewand:

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

The literal strings enter worker identities and provenance. Runtime-owned
variables, including private home and cache directories and protocol channels,
cannot be overridden. Library settings do not establish CPU affinity or enforced
resource isolation.

## Choose what to time

The default `prepare_and_execute` wall time includes loading the shared JSON
data, solving, and serializing the result. Worker startup and imports, shared
dataset generation, independent evaluation, and durable publication are outside
that interval. Coordination and integrity checks between preparation and
execution are included.

The execution policy can select another scope if the problem permits it:

| Scope | What it includes |
| --- | --- |
| `cold_end_to_end` | Startup, preparation, solving, and result serialization. |
| `prepare_and_execute` | Preparation, solving, and result serialization. |
| `execute_only` | Solving and result serialization, with authorized untimed preparation. |

The current problem permits only the default scope. To use another one,
declare its `allowed-timing-scopes` and define what preparation may do outside
the timer. Separate lifecycle callbacks are useful when you need precise
preparation boundaries.

To compare solver computation without Metewand's result serialization, select
the callback clock explicitly:

```yaml
execution-policies:
  solver:
    timing-scope: execute_only
    primary-time: implementation_time
    timeout: 30 s
```

Select `execution-policy: solver` in the experiment. The problem must permit
`execute_only` and define the preparation allowed outside
the timer. The worker measures only its execute callback. Put method-specific
setup there when the comparison charges it. With lifecycle workers, return the
solver object from `execute` and convert it to JSON-compatible values in the
optional `result` callback. Conversion performed inside `execute` still counts.
Metewand's subsequent result conversion, encoding, writing, and transport stay
outside callback time.

Python, R, and Rust adapters report `implementation_time_ns`. R uses elapsed
`proc.time()`, whose resolution depends on the platform; nanosecond units do not
imply nanosecond resolution. A custom worker must report its callback measurement.
A missing measurement fails the attempt with `missing_implementation_time`.

This primary clock is available for one-shot and fresh-sequence execution.
Buffered and streaming profiles use their declared trajectory clocks. The
executor's `timed_wall_time_ns` remains recorded, and scientific timeouts and
fresh-sequence cumulative budgets continue to use that outer wall clock,
including result writing. Reports and exports expose the selected clock through
`primary_time`, `primary_time_ns`, and `primary_time_status`; JSONL rows keep the
clock name in `compatibility.primary_time`.

Linux CPU and peak-memory measurements cover the implementation process, so
their evidence is partial.
These measurements are unavailable on macOS and Windows; missing values are
not zero. Wall time is available on all three platforms.

## Name an execution policy

Add this top-level mapping to the manifest:

```yaml
execution-policies:
  local:
    timing-scope: prepare_and_execute
    primary-time: timed_wall_time
    run-order: randomized
    timeout: 5 s
```

Then add `execution-policy: local` to the experiment. Randomized order
deterministically interleaves implementation configurations within comparison
blocks. Execution remains serial unless you explicitly set `max-concurrency`.
Concurrency changes the workload and is recorded in the execution policy.

`timeout` is a scientific wall-time limit over the selected scope. Separate
operational limits bound each worker phase, with ten-second defaults. A larger
scientific timeout does not extend those phase limits.

Set `worker-timeouts` at the top level to change individual operational limits:

```yaml
worker-timeouts:
  materialize: 10 min
  prepare: 2 min
  execute: 45 s
  evaluate: 1 min
```

The available phases are `hello`, `materialize`, `prepare`, `execute`, `reset`,
`evaluate`, and `shutdown`. Each accepts a positive integer followed by `ms`,
`s`, `min`, or `h`, within the safe nanosecond range. Omitted phases retain
their ten-second limits. Applicability checks use the preparation limit.
Worker checks with `check --workers` use the same settings, including reference
evaluation and shutdown. Materialization limits also bound remote dataset
requests.

Scientific limits still apply to the selected timing scope. For example, a
`30 s` scientific budget with `execute: 45 s` permits an eleven-second solver
call, but still stops measured work at thirty seconds. Preparation remains
within that budget under `prepare_and_execute`; use a contract-authorized
`execute_only` scope for untimed preparation. Effective phase limits are
retained in provenance. Changing them changes resolved execution identities.

Lifecycle workers can opt into `worker-reuse: true` and `warmup-runs: 1`.
Reuse requires a successful reset at each boundary. A replacement process
repeats its warm-ups. Cold timing is incompatible with reuse or warm-ups.

Current runs execute trusted local code. Resource requests are best effort,
and the local backend reports CPU, thread, and memory enforcement as unsupported.
It does not provide network isolation. Ordinary local environments record
development evidence; Nix environments can bind the software closure explicitly.

## Lock Nix worker environments

Declare a Nix environment for every worker used by the plan:

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
App environments use `output-kind: app`, an app name as `output`, and the
matching installable. Their workers must name the app's executable.
Repository scripts use their locked shebang interpreter. Native executables
must come from a realized Nix output.

```sh
metewand lock
metewand lock --check
metewand run --locked --output-dir ../results
```

Preparation may fetch sources and build outputs. Native flake locks must already
be complete; Metewand does not update them. Check in `metewand.lock` with the
benchmark. Root and nested flakes use snapshots of tracked working-tree files,
including uncommitted edits. Stage new source files before locking. The snapshot
excludes `metewand.lock` and Git metadata, so local flakes cannot depend on
`self.rev`. Outside Git, the snapshot includes the whole repository tree, and
run and cache directories must be outside it. Source links, special files, and Git submodules
are unsupported. Worker source bundles must exclude `metewand.lock`.

`lock --check` and `run --locked` verify sources, launch settings, executable
hashes, and the contents and references of the complete runtime closure. They
fail if required paths are missing or changed. Run `metewand lock` again to
prepare missing outputs or refresh changed source evidence. This does not
update native dependencies; use Nix's own tools for that. Locked runs keep
temporary GC roots while executing, but the lockfile does not permanently
prevent Nix garbage collection.

Locked environments remain trusted local processes with the existing execution
controls. They do not establish archival isolation. Dataset acquisition and
worker network access retain their existing behavior. `renv` environments are not yet supported by the lock command.

The [locked Nix example](https://github.com/jolars/metewand/tree/main/examples/locked-nix)
provides a pinned flake and runnable comparison.

## Lock uv worker environments

Select a project or workspace root with an author-owned `uv.lock`:

```yaml
environments:
  python:
    kind: uv
    project: .
    lockfile: uv.lock
    groups: [benchmark]
    extras: []
```

Set the materializer, implementation, and evaluator workers' `environment` to
`python`, and use `runner: python`, function workers, or a command program such
as `python3`. Installed console entry points are also available. Metewand keeps
virtual-environment context and resolves programs within the prepared environment.
Python binding workers also select `protocol-transport: socket`; ordinary function
workers select their adapter automatically.

```sh
uv lock
metewand lock
metewand lock --check
metewand run --locked --output-dir ../results
```

Install uv and a compatible Python interpreter before preparation. Project Python
requirements and `.python-version` select an installed interpreter; Metewand
disables automatic Python downloads. `groups` and `extras` are optional lists.
Omitting them installs base dependencies only, with uv's default groups disabled.
Selections are sorted and deduplicated before hashing. Static checking and
planning do not invoke uv or import benchmark code.

`metewand lock` syncs a separate environment from captured sources with
`--locked`, `--no-editable`, and copied package files. It preserves the native
lock and the author's `.venv`. Project configuration is captured; ambient uv
configuration and Python search paths are excluded. Local dependencies and
workspace members must remain inside captured repository sources. The selected
project or workspace root must contain its declared `uv.lock` and `pyproject.toml`.
Git snapshots use tracked working-tree files, so stage new inputs before locking.
Generated `.venv` directories for declared uv projects are excluded from snapshots.
Keep outputs outside captured sources, as with Nix environments.

Prepared generations live in persistent per-user data storage:

- Linux: `$XDG_DATA_HOME/metewand`, or `~/.local/share/metewand`.
- macOS: `~/Library/Application Support/metewand`.
- Windows: `%LOCALAPPDATA%/metewand`.

Override the root with an absolute `METEWAND_DATA_DIR` outside captured sources.
Use the same root for preparation and locked execution. Prepared generations
retain their installation paths and are not portable virtual environments.
Preparation reuses verified generations and never overwrites an older one.

For uv-only benchmarks, `lock --check` and `run --locked` work offline without uv
or Nix installed. They verify source content, the interpreter and standard
library, installed files and symlink targets, platform, and worker launches.
Missing, changed, or added installed files cause preflight failure before run
output is created. Verification does not fetch, build, sync, or repair anything.
Run `metewand lock` explicitly to prepare missing state or rebuild changed local
sources, including source-only edits that leave package metadata unchanged.
Mixed Nix and uv benchmarks verify each backend with its own requirements.

Nix/uv-only Metewand locks use version 2, with backend-specific evidence.
OCI-containing locks use version 3. Apptainer-containing locks use version 4. Version-1 Nix
locks remain readable and retain their original identities. uv identities
normalize generated metadata and launcher paths; different built package content
still defines different resolved evidence. Raw inventories verify exact installed
bytes independently of these normalized identities. Workers use a private
bytecode cache prefix, so ordinary host bytecode cache changes do not invalidate
the locked runtime.

The evidence covers the prepared Python runtime and packages. It does not bind
every external native library or establish a reproducible build toolchain.
Installed native extensions are verified as files, and preparation records uv
and build settings. Workers retain the trusted local executor's reported network
and filesystem controls. See the [locked uv
example](https://github.com/jolars/metewand/tree/main/examples/locked-uv).

## Keep the run outside source bundles

```sh
metewand check --workers
metewand plan
metewand run
metewand status
```

`metewand run` saves its durable history in `.metewand/` beside the manifest,
including when you select a manifest with `--manifest PATH`. Add `.metewand/` to
your repository's `.gitignore`. Use `--output-dir PATH` to choose another run;
relative paths are resolved from the working directory, and the parent must
exist. A new directory starts a run; an existing Metewand run directory resumes
automatically. Other existing directories are rejected.

The run directory and its contents must stay outside the declared source
selection. The walkthrough includes only `benchmark.py` as worker source, so
the default directory is suitable. A selection such as `benchmark/*.py` also
allows `benchmark/outputs/run`, since neither that directory nor its contents can
match the glob. A matched directory includes its
whole tree: `benchmark/output.py/run` would overlap `benchmark/*.py`. If you
declare the whole benchmark directory or use `benchmark/**/*.py`, choose an
output directory outside that selection with `--output-dir`. The same selection
rules apply to an explicit artifact cache.

By default, `run` finishes with a short summary and the output directory. Add
`--verbose` to print individual measurements, effective settings, metrics,
completion, failures, and profile details. To inspect saved measurements without
rerunning the benchmark, use `metewand status`. This inspects the current manifest
and the default run when it exists. Use `--output-dir PATH` to inspect another run
independently of the manifest. In a toy Python benchmark, serialization and
orchestration costs can be substantial
relative to the solver's work.

Human runs report preparation immediately, including environment verification,
source staging, dataset generation or cache verification, shared problem inputs,
and applicability checks. Execution messages identify the experiment, dataset,
implementation, repetition, and measurement. The display counts pending work for
this invocation, so resume skips and `--max-attempts` are reflected in progress.
Failures include their diagnostic codes.

Interactive terminals show preparation spinners, an overall progress bar, and a
separate bar for each combination of experiment, dataset, and implementation.
Bars share aligned columns and show completed attempts, totals, and percentages.
The display also shows elapsed time and active attempts. Completed preparation
and benchmark sections fold into summaries; failure diagnostics remain visible.
Bar totals come from the actual pending schedule after resume and selection;
`--max-attempts` bounds the displayed work for this invocation. Colors distinguish
preparation, accepted results, failures, and other outcomes. Set `NO_COLOR` to
disable color.

With redirected stdout or stderr, or `TERM=dumb`, progress uses plain lines and
emits a heartbeat every ten seconds during long tasks. No progress escape
sequences enter redirected logs. Progress goes to stderr; the final summary and
optional comparison go to stdout. The comparison table adapts to the terminal
width up to 110 columns, leaves a right margin, aligns timing values to the
right, and prints settings and metrics shared by all rows above the table.
Preparation messages and heartbeats are transient display information, not
persisted scientific evidence.

For structured output, use `--output json`. Use `--output jsonl` to stream
versioned execution events after preparation completes. These modes disable the
human display entirely and retain complete records regardless of `--verbose`.
Worker logs stay separate from the command's machine
output.

## Resume and diagnose

```sh
metewand run
metewand run --retry
metewand status --output json
```

Repeat the same command to fill in remaining work. Metewand verifies the resolved
identities and skips matching accepted measurements. Changes to sources, inputs,
environments, or policies create new identities for affected measurements. Use a
new output directory for an independent run of the same benchmark. `--resume`
remains accepted as a compatibility alias for the default behavior.

Press Ctrl+C before shutting down, then repeat the command when you return.
Metewand recovers interrupted attempts and starts them again from clean state,
preserving their earlier evidence. Recovery may need the old ownership lease to
expire, currently up to 30 seconds. Attempts still owned by another process are
left alone unless you explicitly request a retry.

Recorded execution or scientific failures require `--retry`. It authorizes one
additional attempt for each selected failed slot during that invocation and
preserves earlier attempts. A repeated command does not automatically retry those
failures.

Copy an ID from `status` to inspect its recorded evidence:

```sh
metewand explain ID
```

Replace `ID` with a full attempt, observation, diagnostic, or other supported
record ID. Use `--output-dir PATH` to inspect stored evidence without the
manifest. Inspection does not recover or modify the run. Use `--select ID` on
`run` to narrow execution, or `--max-attempts 2` to bound the new attempts in one
invocation.

An execution exception, malformed result, evaluator failure, scientific timeout,
and missed accuracy target represent different problems. Read the diagnostic
before retrying: unchanged deterministic code will often fail in the same way.

## Export for analysis

```sh
metewand export --output-dir .metewand --format json --require-complete --destination analysis
metewand export --output-dir .metewand --format csv --destination tables
metewand export --output-dir .metewand --destination jsonl-tables
```

Each new destination contains the selected results file and `export.json`.
The latter records selected identities, compatibility evidence, and the table's
hash. Exports read the stored evidence, not the original source checkout.

For plotting or retaining a documentation snapshot, choose `--format json`. Its
`results.json` contains one object per observation with a full `metrics`
dictionary, method names, effective settings, repetition indices, profile
controls, and separate timing coordinates. `implementation_time_ns` is the
worker's fit timer; `timed_wall_time_ns` is the executor's timer. Profile clocks
remain separate. `primary_time_ns` selects the configured comparison clock
without substituting another measurement when it is unavailable. The snapshot
also includes attempts, registered slots,
applicability, profile progress, and a `completeness` result.

Each observation's `provenance_id` selects recorded evidence in the snapshot's
`provenance` dictionary. It retains the dataset derivation and content hash,
host observations, runner executable evidence, and worker environments. Git
revision and dirty status describe the checkout at run preflight. Package
versions and native library details can be returned by a worker's `statistics`
callback and appear on each observation. Historical unavailable evidence remains
unavailable; export does not inspect the exporting host or current checkout.

`--require-complete` checks declared repetitions, registered measurements, and
profile completion before writing any export or rebuilding an index. Successful
retries and legitimate target-based skips satisfy it. Valid intermediate profile
points remain present, including those marked `not_reached`. An unfinished or
exhausted comparison returns `incomplete_results` with exit code 5. With `--id`,
the check covers the registered measurement scope of the selected candidates.
Excluded candidates remain visible and do not become successful measurements.

To plot accepted measurements, select observations whose `selected` field is
true and `warmup` field is false. Choose aggregation and figure-specific checks
in your analysis code. Keep the full run directory or create an archive when you
need result artifacts and complete coordination history.

Tables contain one row per evaluator metric coordinate on an observation, plus
rows for entities without metrics. For this problem, an observation can
therefore have separate `/objective` and `/gradient_inf` rows. Do not count
those as separate repetitions. Filter by `record_type` to choose observations,
and use attempt and observation IDs to group related rows. `metric_value` and
nested evidence cells contain JSON, including in CSV.

Keep failed attempts and completion misses in the analysis. Match datasets,
problem settings, timing scopes, and execution evidence before comparing
durations. A matching compatibility key does not establish equal hardware or
scientific comparability. Choose aggregation and statistical summaries in your
analysis tools.

## Share stored results

Create an archive after every started attempt has finished. Failed attempts and
pending slots may remain. Creation reads committed evidence and leaves the run
unchanged; it does not recover interrupted attempts.

```sh
metewand archive create --output-dir .metewand --destination results.tar.gz
metewand archive verify --archive results.tar.gz
metewand archive import --archive results.tar.gz --destination imported
metewand status --output-dir imported
metewand export --output-dir imported --format csv --destination shared-tables
```

The archive retains stored records, results, evaluator metrics, profile snapshots
and traces, failure history, and available provenance. It preserves external
source and dataset identities without fetching their bytes. Verification works
offline and checks the evidence's consistency and integrity without running
benchmark code. It does not rerun the scientific evaluation or establish who
produced the evidence.

An imported collection supports `status`, `explain`, and `export` without the
original checkout, cache, or run directory. It is read-only: resume, recovery,
and `--rebuild-index` are unavailable. Export tables to a new directory outside
the collection. Archive and import destinations must be new paths with existing
parents; put the archive outside its source run or collection.

Verify an imported collection again with `metewand archive verify --archive
imported`. Verification and import allow up to one million files and 1 TiB of
expanded payload by default, including metadata. Set `--max-entries N` and
`--max-bytes N` to choose different limits. The target filesystem must support the
stored filenames; import reports unsupported names or collisions without changing
them. Matching archive contents preserve the recorded execution guarantees and
timing scopes.

The next part covers [other languages, artifacts, and profiles](extending.md).
