# Streaming profiles

Run a halving trajectory against the objective `x²`. The worker serializes a
checkpoint, blocks while an independent evaluator computes `gap`, and follows
the runner's `continue` or `stop` decision. Python and native Rust use the same
protocol and result envelopes.

Stage the example and package from the repository root:

```sh
benchmark_dir="$(mktemp -d)"
cp -R examples/streaming-profile/. "$benchmark_dir/"
cp -R packages/python/src/metewand "$benchmark_dir/metewand"
compute run -- cargo run --locked -p metewand -- check --manifest "$benchmark_dir/metewand.yaml" --workers
compute run -- cargo run --locked -p metewand -- plan --manifest "$benchmark_dir/metewand.yaml" --output json
compute run -- cargo run --locked -p metewand -- run --manifest "$benchmark_dir/metewand.yaml" --output-dir "$benchmark_dir/run" --output json
compute run -- cargo run --locked -p metewand -- status --output-dir "$benchmark_dir/run"
compute run -- cargo run --locked -p metewand -- export --output-dir "$benchmark_dir/run" --destination "$benchmark_dir/jsonl" --format jsonl
compute run -- cargo run --locked -p metewand -- export --output-dir "$benchmark_dir/run" --destination "$benchmark_dir/csv" --format csv
compute run -- cargo run --locked -p metewand -- run --manifest "$benchmark_dir/metewand.yaml" --output-dir "$benchmark_dir/run" --resume
```

Expect three accepted attempts and thirteen independently evaluated checkpoints.
Two target trajectories stop after five checkpoints each; the fixed-count
trajectory stops after three. Warmups use the same path without publishing
measured observations. The evaluator's one-shot completion verdict remains
`not_reached` for nonzero iterates. The profile applies its own metric threshold
after checking scientific validity.

For the native loop, build and stage its executable:

```sh
compute run -- cargo build --locked -p metewand --example streaming-worker
cp target/debug/examples/streaming-worker "$benchmark_dir/native-worker"
compute run -- cargo run --locked -p metewand -- check --manifest "$benchmark_dir/native.yaml" --workers
compute run -- cargo run --locked -p metewand -- run --manifest "$benchmark_dir/native.yaml" --output-dir "$benchmark_dir/native-run"
```

On Windows, copy `streaming-worker.exe` as `native-worker.exe`, and update the
`program` and `sources` entries in `native.yaml` to use that `.exe` filename.

The binding declares `delivery: streaming_profile`,
`measurement-basis: executor_observed`, and a `check-schedule`. Common policy
fields bound observations and active time; `max-checkpoint-bytes` bounds each
snapshot's envelope and files. `record-thresholds` annotate crossings computed
from evaluator metrics. Every checkpoint is published and counts against
`max-observations`, including checkpoints that cross no threshold.

In Python, register `execute_stream(state, context)` and call
`context.stream.checkpoint(progress, snapshot)`. The snapshot can be a callable
returning a dictionary or `Result`. The helper invokes it only on scheduled
checks. Write custom files under `context.stream.output_dir` inside that callable
and return their `Artifact` declarations. `finish(progress, snapshot)` can record
an off-schedule final point. Rust exposes the same operations through
`StreamingContext`, including `checkpoint_with` for custom writers. Quiesce all
solver work, including threads and child processes, before entering a checkpoint.

`active_time_ns` includes checkpoint serialization and managed-output
finalization. It excludes validation, evaluation, publication, and the final
stop acknowledgment. `timed_wall_time_ns` carries this active coordinate;
elapsed wall time and cumulative evaluation time remain separate. Linux CPU
uses partial process-group sampling, which can miss short-lived or detached
children. Windows CPU uses cumulative Job Object totals. macOS CPU remains
unavailable. These are local development guarantees.

A timeout, invalid later result, or worker failure retains earlier published
checkpoints. Inspect and export partial curves with their attempt IDs and
selection status. Use `--resume --retry` to authorize a complete clean-state
retry. Acceptance selects one producing attempt atomically. Retained checkpoints
do not restore solver state or combine with a retry's trajectory.
