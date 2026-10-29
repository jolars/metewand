# 5. Run and analyze the comparison

Use `metewand plan` after each manifest change to inspect the expanded settings.
Start with a small run before increasing the grid or repetition counts.

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

In-worker `implementation_time_ns`, when present, measures the solve callback.
It supplements the primary executor-observed time. Linux CPU and peak-memory
measurements cover the implementation process, so their evidence is partial.
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

Lifecycle workers can opt into `worker-reuse: true` and `warmup-runs: 1`.
Reuse requires a successful reset at each boundary. A replacement process
repeats its warm-ups. Cold timing is incompatible with reuse or warm-ups.

Current runs execute trusted local code. Resource requests are best effort,
and the local backend reports CPU, thread, and memory enforcement as unsupported.
It does not provide network isolation or a complete locked software environment.

## Keep the run outside source bundles

```sh
metewand check --workers
metewand plan
metewand run --output-dir results
metewand status --output-dir results
```

The output directory must be new, its parent must exist, and it must be outside
declared source directories and glob prefixes. The walkthrough includes only
`benchmark.py` as worker source, so `results` beside it is suitable. If you
declare the whole benchmark directory as a source, put output elsewhere.

The comparison retains method names, effective settings, repetitions, metrics,
completion, and failures. It reports individual measurements. In a toy Python
benchmark, serialization and orchestration costs can be substantial relative to
the solver's work.

For structured output, use `--output json`. Use `--output jsonl` to stream
versioned events as execution advances. Human progress goes to standard error;
worker logs stay separate from the command's machine output.

## Resume and diagnose

```sh
metewand run --output-dir results --resume
metewand run --output-dir results --resume --retry
metewand status --output-dir results --output json
```

Resume skips accepted slots. `--retry` authorizes one additional attempt for
each selected failed slot during that invocation. It preserves the earlier
attempts. A worker killed during execution can require its ownership lease to
expire before recovery, currently up to 30 seconds.

Copy an ID from `status` to inspect its recorded evidence:

```sh
metewand explain ID --output-dir results
```

Replace `ID` with a full attempt, observation, diagnostic, or other supported
record ID. Stored inspection does not need the manifest and does not recover
or modify the run. Use `--select ID` on `run` to narrow execution, or
`--max-attempts 2` to bound the new attempts in one invocation.

An execution exception, malformed result, evaluator failure, scientific timeout,
and missed accuracy target represent different problems. Read the diagnostic
before retrying: unchanged deterministic code will often fail in the same way.

## Export for analysis

```sh
metewand export --output-dir results --format csv --destination tables
metewand export --output-dir results --destination jsonl-tables
```

Each new destination contains `results.csv` or `results.jsonl` and `export.json`.
The latter records selected identities, compatibility evidence, and the table's
hash. Exports read the stored evidence, not the original source checkout.

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
metewand archive create --output-dir results --destination results.tar.gz
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
