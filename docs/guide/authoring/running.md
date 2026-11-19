# Run and analyze the comparison

Keep using the least-squares benchmark from the previous pages. You have two
methods that solve the same problem and an evaluator that checks their answers.
Now you can run the comparison, examine the measurements, and export them for
analysis.

## Check the plan {#check-and-start-the-run}

From the directory containing `metewand.yaml`, run:

```sh
metewand check --workers
metewand plan
```

The first command checks the manifest and starts its workers to check that they
can respond correctly. The second shows what Metewand will run. Look for the
methods and datasets you intended to compare, and check how many measurements
you have requested.

A small change to a parameter grid can create a lot more work. Before starting a
large comparison, try a small dataset and a few repetitions. This gives you a
chance to find problems in the code or evaluator while the run is still quick.

## Run the comparison {#keep-the-run-outside-source-bundles}

Save this run in a directory named `comparison-run`:

```sh
metewand run --output-dir comparison-run
```

Choose a new directory for an independent run. Metewand creates it, but its
parent must already exist. If you use an existing Metewand run directory,
Metewand continues that run instead.

Without `--output-dir`, results go in `.metewand/` beside the manifest. Add that
directory to `.gitignore` if you use it. Keep run directories outside the files
you include as worker sources. The walkthrough includes only `benchmark.py`, so
`comparison-run` beside it is suitable.

The command shows its progress and finishes with a summary. A method can finish
running yet fail the comparison because its answer misses the accuracy target.
The measurements are still saved, so you can inspect what happened.

## Understand what gets timed {#choose-what-to-time}

The default timer starts when the worker begins preparing the computation. It
includes loading the shared data, running the method, and converting and writing
the result as JSON. Metewand calls this scope `prepare_and_execute`.

Starting the worker and importing its packages happen before this timer starts.
Generating the shared dataset and independently evaluating the answer also
happen outside the timed interval. Both methods receive the same generated data.

This matters when you interpret a small benchmark. For these tiny inputs,
loading data and writing the answer may take a substantial part of the measured
time. The number describes the work inside the timed interval, which includes
more than the solver's arithmetic.

Use the same timing definition for the methods you compare. If your question
specifically concerns the solver's own computation, see the optional recipe for
[timing the solver](execution.md).

## Read the results {#read-the-output}

Inspect the saved run:

```sh
metewand status --output-dir comparison-run --verbose
```

`status` reads the saved measurements without running the methods again. The
verbose output shows the methods, settings, timings, metrics, and failures.

For this example, look at `objective` and `gradient_inf` alongside the timing.
The objective measures the least-squares loss. The gradient metric is the
largest absolute component of the gradient, which the evaluator compares with
the accuracy target you declared.

If you kept the iteration-limit case from the previous pages, expect it to miss
that target. A coefficient vector can be a valid answer while still being too
inaccurate for the comparison. A fast attempt that misses the target should not
be reported as a successful solution.

## Repeat measurements

A single timing can vary because of other work on the machine. Repeat the same
configured computation to see how much the measurements vary. If you have not
already done so on the [parameters page](parameters.md), add this field to the
existing experiment:

```yaml
measurement-repetitions: 3
```

Keep the experiment's other fields, then run it again:

```sh
metewand plan
metewand run --output-dir comparison-run
```

Metewand keeps matching completed measurements and adds the repetitions that
remain. You do not need to discard the run to request more measurements. Retain
the individual timings when you analyze variation. A retry after a failure is a
separate attempt to obtain a result, rather than an extra measurement
repetition.

### Adapt the number of measurements

For a one-shot experiment, set a minimum and maximum number of measurements and
a target for cumulative measured time.

```yaml
measurement-repetitions:
  min: 5
  max: 30
  min-time-seconds: 2
```

Metewand samples each implementation configuration and seed independently. Once
it has accepted at least five measurements for a series, it stops that series
when their primary timings add up to two seconds or when it accepts the
thirtieth measurement, whichever happens first. The time target uses the clock
selected by `primary-time` and may be fractional. A failed measurement pauses
its series until that slot is retried successfully. Warmups and failed attempts
do not contribute to the count or the time.

`metewand plan` shows all possible slots up to the maximum. The runner registers
one slot at a time for each adaptive series. Resuming uses the accepted
measurements already recorded, and `metewand status` shows the accepted count
and any stopping reason. Changing the adaptive settings starts a new series.
Selecting an individual adaptive slot by ID selects its entire series because
earlier measurements determine whether that slot runs.

## Stop and continue {#resume-and-diagnose}

If you need to stop, press Ctrl+C. When you return, repeat the command:

```sh
metewand run --output-dir comparison-run
```

Metewand keeps completed measurements and restarts interrupted attempts from the
beginning. It reuses results only when the inputs, code, environments, and
policies still match. Keep those unchanged when you want to continue the same
comparison. Choose a new output directory when you want to start independently.

### Investigate a failure

An attempt that finishes with a recorded failure is not retried automatically.
Start by reading its diagnostic in `status`. Copy the record ID into:

```sh
metewand explain ID --output-dir comparison-run
```

For example, `completion_target_not_reached` means the evaluator found a valid
answer that missed the declared target. In the iteration-limit case, the method
was only allowed one iteration. Running it again with that same limit may
produce the same outcome.

After investigating, you can explicitly request another attempt:

```sh
metewand run --output-dir comparison-run --retry
```

This allows one new attempt for each selected failed measurement. Metewand keeps
the earlier attempts, so you can see the failure history when reviewing the
results.

## Export and analyze the measurements {#export-for-analysis}

Export the run as a CSV table:

```sh
metewand export --output-dir comparison-run --format csv --destination tables
```

Open `tables/results.csv` in your analysis tool. The destination must be a new
directory, and its parent must already exist.

The table includes several kinds of record. Rows with `record_type` set to
`observation` contain the measured results. Check their `completion` values when
comparing methods that have an accuracy target.

One observation can occupy several rows. In this example, `/objective` and
`/gradient_inf` are two metrics from the same measurement. Use the observation's
ID to keep those rows together. They are not two separate repetitions.

Keep failures and missed accuracy targets in your report. Before comparing
timings, check that the methods solved the same problem on the same data with
the same timing definition. Compare accuracy alongside speed, and retain the
individual repetitions when assessing variation.

If you prefer structured data, export JSON instead:

```sh
metewand export --output-dir comparison-run --format json --destination analysis
```

`analysis/results.json` keeps each observation's metrics together in one object.
It also includes attempts and failures. Choose the format that is easier to use
in your analysis code. Export reads the saved run, so you can revisit the
analysis without running the methods again.

## When you need more {#change-how-workers-run}

<span id="share-stored-results"></span>

To send the run to someone else or save it for recovery, [create an
archive](results.md#share-stored-results). An archive keeps the recorded results
and their files together.

<span id="name-an-execution-policy"></span>

If the default timer does not answer your scientific question, [time the
solver](execution.md).

<span id="declare-worker-variables"></span>
<span id="lock-nix-worker-environments"></span>
<span id="lock-uv-worker-environments"></span>

To record and verify the software used by workers, [prepare a locked
environment](environments.md).

<span id="select-a-named-experiment"></span>
<span id="filter-solvers-and-datasets"></span>

Use `metewand run --help` for other command options. The next part covers [other
languages, artifacts, and profiles](extending.md).
