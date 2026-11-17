# Analyze and share results

Metewand keeps individual measurements, metrics, failures, and the records
needed to trace them back to the run. Export them for analysis, or create an
archive to share the results and their artifacts.

## Export a table {#export-for-analysis}

Choose a new directory whose parent already exists:

```sh
metewand export --output-dir .metewand --format csv --destination tables
metewand export --output-dir .metewand --destination jsonl-tables
```

JSONL is the default format. Each destination contains the results file and
`export.json`, which records the selected identities, compatibility evidence,
and the table's hash. Export reads the stored records without needing the
original checkout.

Tables contain one row per metric coordinate on an observation, plus rows for
records without metrics. In the least-squares example, a measurement can have
separate `/objective` and `/gradient_inf` rows. These are two metrics from the
same measurement. Use attempt and observation IDs to group related rows, and
filter `record_type` to choose observations. `metric_value` and nested cells
contain JSON, including in CSV.

## Save a JSON snapshot

For plotting or keeping a snapshot with your documentation, use:

```sh
metewand export --output-dir .metewand --format json --require-complete --destination analysis
```

`results.json` contains one object per observation, with its `metrics`
dictionary, method name, settings, repetition indices, and profile controls. It
also includes attempts, registered measurements, applicability, profile
progress, and a `completeness` result.

Timing fields retain their separate meanings:

  | Field                    | Meaning                                                  |
  | ------------------------ | -------------------------------------------------------- |
  | `implementation_time_ns` | The worker's timer around its solver callback.           |
  | `timed_wall_time_ns`     | The executor's wall timer.                               |
  | `primary_time_ns`        | The configured comparison clock's value, when available. |

Metewand never substitutes another clock for a missing primary measurement.
Profile clocks also remain separate. See [choose what to
time](execution.md#choose-what-to-time) before comparing measurements made with
different timing settings.

Each observation's `provenance_id` points to an entry in the snapshot's
`provenance` dictionary. That entry records how the dataset was derived, its
content hash, host observations, the runner executable, and worker environments.
Git revision and dirty status describe the checkout when the run started.
Workers can return package versions and native library details through their
`statistics` callback; these appear on each observation. Export leaves missing
historical evidence unavailable and does not inspect the exporting host or
current checkout to fill it in.

## Check that the comparison finished

Use `--require-complete` when your analysis needs a finished comparison. It
checks declared repetitions, registered measurements, and profile completion
before writing an export or rebuilding an index. Successful retries and
legitimate skips after reaching a target satisfy the check. Valid intermediate
profile points remain present, including those marked `not_reached`.

An unfinished or exhausted comparison returns `incomplete_results` with exit
code 5. With `--id`, the check covers measurements registered for the selected
candidates. Excluded candidates remain visible and do not count as successful
measurements.

## Compare measurements carefully

In a JSON snapshot, select observations whose `selected` field is true and
`warmup` field is false when plotting accepted measurements. Choose aggregation
and figure-specific checks in your analysis code. Keep the full run directory or
an archive when you need result artifacts and the complete attempt history.

Keep failed attempts and missed accuracy targets in the analysis. Check that
datasets, problem settings, timing scopes, and execution evidence are suitable
for the comparison before comparing durations. Matching compatibility keys do
not establish equal hardware or scientific comparability. Retain individual
repetitions when assessing variation, and choose statistical summaries in your
analysis tools.

## Share stored results

Create an archive after every started attempt has finished. Failed attempts and
pending work may remain. Archive creation reads committed records and leaves the
run unchanged; it does not recover interrupted attempts.

```sh
metewand archive create --output-dir .metewand --destination results.tar.gz
metewand archive verify --archive results.tar.gz
metewand archive import --archive results.tar.gz --destination imported
metewand status --output-dir imported
metewand export --output-dir imported --format csv --destination shared-tables
```

The archive keeps stored records, results, evaluator metrics, profile snapshots
and traces, failure history, and available provenance. It preserves external
source and dataset identities without fetching their bytes.

Verification works offline. It checks the records' consistency and integrity
without running benchmark code. It does not repeat the scientific evaluation or
establish who produced the evidence.

An imported collection supports `status`, `explain`, and `export` without the
original checkout, cache, or run directory. It is read-only: resume, recovery,
and `--rebuild-index` are unavailable. Export tables to a new directory outside
the collection. Archive and import destinations must be new paths with existing
parents. Put the archive outside its source run or collection.

Verify an imported collection again with
`metewand archive verify --archive imported`. Verification and import allow up
to one million files and 1 TiB of expanded payload by default, including
metadata. Use `--max-entries N` and `--max-bytes N` to change those limits. The
destination filesystem must support the stored filenames; import reports
unsupported names or collisions without changing them. Matching archive contents
preserve the recorded execution guarantees and timing scopes.

## Continue from an archive

If you lose the original local run directory, restore an archive to a new
directory on supported local storage:

```sh
metewand archive restore --archive results.tar.gz --destination restored
metewand status --output-dir restored
metewand run --manifest metewand.yaml --output-dir restored
```

Restore verifies the archive, rebuilds the coordination database, and keeps
accepted measurements, pending work, failures, retries, and profile accounting.
It does not launch workers. Continuing requires the original benchmark sources,
inputs, and compatible prepared environments. Repeat the original experiment and
selection flags. Recorded failures still need `--retry`; matching completed
measurements are reused.

Keep the archive for recovery or sharing. The restored run can acquire new
history independently. On a cluster, keep the archive in shared project storage
and restore to supported local storage. Save an archive after started attempts
finish and before local scratch is removed. Restore requires a filesystem that
can preserve the stored executable bits; use read-only import for analysis when
that is unavailable.

Return to [running the comparison](running.md), or [extend the
benchmark](extending.md) with other languages, artifacts, and profiles.
