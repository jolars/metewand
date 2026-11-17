# Run and analyze the comparison

Start with a small run. Check that the methods produce useful answers before
increasing the input sizes or repetition counts.

## Check and start the run

From the directory containing `metewand.yaml`, run:

```sh
metewand check --workers
metewand plan
metewand run
metewand status
```

`check --workers` validates the manifest and checks that its workers can run.
`plan` shows which methods and datasets will run, how many measurements to
expect, and which timing settings apply. Review it after each manifest change.
`run` performs the comparison; `status` reads the saved results.

By default, Metewand times data loading, solving, and result serialization.
Worker startup, dataset generation, and independent evaluation happen outside
that interval. See [choose what to time](execution.md#choose-what-to-time) when
your comparison needs a different timer.

## Choose where to save results {#keep-the-run-outside-source-bundles}

Metewand saves the run in `.metewand/` beside the manifest, including when you
choose a manifest with `--manifest PATH`. Add `.metewand/` to `.gitignore`. To
keep a separate run, choose another directory:

```sh
metewand run --output-dir ../results
metewand status --output-dir ../results
```

Relative paths start from your working directory, and the parent must exist. A
new directory starts a run. An existing Metewand run directory continues that
run automatically. Other existing directories are rejected.

Keep the run outside the files you include as worker sources. This walkthrough
includes only `benchmark.py`, so `.metewand/` is suitable. If you include the
whole project directory, save the run outside it. The same rule applies to an
explicit artifact cache. See [include the files you want to
lock](environments.md#include-the-files-you-want-to-lock) for source patterns
and locked environments.

## Choose an experiment {#select-a-named-experiment}

A manifest can keep small and full comparisons as separate experiments. Choose
one by name:

```sh
metewand plan smoke
metewand check smoke --workers
metewand run smoke
```

Set `default-experiment: full` at the top level to choose the usual experiment.
Without a default or an explicit choice, commands select every experiment.

`metewand run --experiment smoke` means the same thing. You can supply several
names, such as `metewand run smoke full`, repeat `--experiment`, or mix the two
spellings. Metewand runs the selected experiments in manifest order. An explicit
choice overrides the default. Repeat your choice when continuing the run.

For a benchmark with its own manifest, use:

```sh
metewand run --manifest benchmarks/gaussian-regression/metewand.yaml
```

## Choose methods and datasets {#filter-solvers-and-datasets}

Use the same filters to preview, check, and run a smaller comparison:

```sh
metewand plan --dataset simulated --implementation gradient_descent
metewand check --dataset simulated --implementation gradient_descent --workers
metewand run --dataset simulated --implementation gradient_descent
```

Use `--problem` to choose a problem too. Names must match the manifest exactly,
including case. Repeat a flag to include several names. For example,
`--implementation gradient_descent --implementation coordinate_descent` includes
both methods. Using both a dataset and an implementation filter requires a match
for both. Unknown names or a combination that selects no work cause an error
before execution.

Metewand chooses experiments first, then applies these filters. Each selected
method and dataset keeps its parameter choices and repetitions. Repeated
`--select ID` flags on `run` can narrow the selection further; an ID excluded by
the other filters is an error.

Static checks still validate the whole manifest. `check --workers` launches only
the workers needed for your selection. Repeat the filters when continuing the
same subset, or broaden them to run more methods and datasets. Matching accepted
measurements remain reusable.

## Read the output

`run` ends with a short summary and the output directory. Add `--verbose` to see
individual measurements, settings, metrics, and failures. Use `status` to
inspect the saved results without running workers again. With
`--output-dir PATH`, it can inspect another run without the manifest.

During a run, Metewand reports preparation and shows which method, dataset, and
repetition it is working on. Progress reflects the work still to do in this
invocation. Redirected logs receive plain updates and a heartbeat every ten
seconds during long tasks. Progress goes to stderr; the final summary goes to
stdout. Set `NO_COLOR` to disable color.

For scripts, choose structured output:

```sh
metewand plan --output json > plan.json
metewand run --output jsonl > events.jsonl
```

JSON and JSONL contain complete records regardless of `--verbose`. JSONL streams
execution events after preparation completes. These modes disable the human
display and keep worker logs separate. For a fuller human-readable plan, use
`metewand plan --verbose`; `metewand plan --output jsonl` also exports the plan.

## Resume and diagnose

Repeat the same command to continue a run. Metewand checks the inputs, code,
environments, and policies, then skips matching accepted measurements. Changed
inputs create new identities for the affected measurements. Use a new output
directory when you want an independent run of the same benchmark. `--resume`
remains accepted as an alias for the default behavior.

If you need to stop, press Ctrl+C. Repeating the command restarts interrupted
attempts from clean state and keeps their earlier records. Recovery may wait up
to 30 seconds for the previous process's ownership lease to expire. Attempts
owned by another process are left alone unless you explicitly request a retry.

Recorded failures need an explicit retry:

```sh
metewand run --retry
```

This allows one new attempt for each selected failed measurement and preserves
the earlier attempts. Read the diagnostic first: an exception, malformed result,
timeout, evaluator failure, and missed accuracy target need different fixes.
Repeating unchanged code may produce the same failure.

Copy an ID from `status` to inspect its records:

```sh
metewand explain ID
```

`ID` can identify an attempt, observation, diagnostic, or another supported
record. Add `--output-dir PATH` to inspect a run without its manifest.
Inspection does not modify or recover the run. Use `--max-attempts 2` on `run`
to limit how much new work one invocation starts.

## Export the results {#export-for-analysis}

Export a table to a new directory whose parent already exists:

```sh
metewand export --output-dir .metewand --format csv --destination tables
```

Compare accuracy alongside timing, and keep failed attempts in your analysis. A
single measurement can produce several metric rows. See [analyze and share
results](results.md) for how to read the tables, choose a timer, and check that
the comparison finished.

<span id="share-stored-results"></span>

To share results or keep a copy for recovery, [create an
archive](results.md#share-stored-results). It keeps the recorded results and
artifacts as well as the history of attempts.

## Change how workers run

<span id="choose-what-to-time"></span>
<span id="name-an-execution-policy"></span>

[Choose how the comparison runs](execution.md) when you need to change the
timer, run order, time limits, or worker reuse.

<span id="declare-worker-variables"></span>
<span id="lock-nix-worker-environments"></span>
<span id="lock-uv-worker-environments"></span>

[Prepare worker environments](environments.md) to set library variables or lock
the software used by Python and Nix workers.

The next part covers [other languages, artifacts, and profiles](extending.md).
