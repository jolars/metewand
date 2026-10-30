# Release orchestration and serialization overhead

This controlled native worker performs 0, 1, or 10 ms of monotonic-clock-bounded
integer work, then returns an empty or 1,000,000-byte ASCII payload. An independent
native evaluator checks the answer and payload. Eleven repetitions cover each
combination under all three primary wall-time scopes.

Build the runner and worker with receipts from the repository root:

```sh
python3 tools/build_evidence.py --executable target/release/metewand \
  --source metewand=. --compiler rustc --profile release --lockfile Cargo.lock -- \
  cargo build --release --locked -p metewand --bin metewand
python3 tools/build_evidence.py --executable target/release/examples/overhead-worker \
  --source metewand=. --compiler rustc --profile release --lockfile Cargo.lock -- \
  cargo build --release --locked -p metewand --example overhead-worker
benchmark_dir=$(mktemp -d)
cp -R examples/overhead/. "$benchmark_dir/"
cp target/release/examples/overhead-worker "$benchmark_dir/worker"
cp target/release/examples/overhead-worker.metewand-build.json "$benchmark_dir/worker.metewand-build.json"
metewand_bin="$PWD/target/release/metewand"
"$metewand_bin" check --manifest "$benchmark_dir/metewand.yaml" --workers
"$metewand_bin" plan --manifest "$benchmark_dir/metewand.yaml" --output json > "$benchmark_dir/plan.json"
time "$metewand_bin" run --manifest "$benchmark_dir/metewand.yaml" --output-dir "$benchmark_dir/run"
"$metewand_bin" status --output-dir "$benchmark_dir/run"
"$metewand_bin" export --output-dir "$benchmark_dir/run" --destination "$benchmark_dir/jsonl"
"$metewand_bin" export --output-dir "$benchmark_dir/run" --format csv --destination "$benchmark_dir/csv"
python3 examples/overhead/analyze.py --plan "$benchmark_dir/plan.json" \
  --jsonl "$benchmark_dir/jsonl/results.jsonl" --destination "$benchmark_dir/measurements.json"
```

The worker diagnoses computation, result encoding, result file writing, and
elapsed work through result writing and diagnostic assembly separately. Encoding includes allocating the payload,
building the result envelope, canonical validation, and JSON encoding. Writing
covers the result file write and close; it does not include durable publication.
`implementation_time_ns` covers scalar control lookup and the controlled computation. These worker
clock readings remain diagnostics.

`timed_wall_time_ns` is always authoritative. `execute_only` covers the execute
exchange and complete result writing. `prepare_and_execute` also includes the
continuous preparation exchange and intervening integrity checks. Cold timing
includes worker launch and handshake. Noncold runs reuse workers and perform
one warm-up. The envelope minus the worker's execute duration is a diagnostic
residual: it includes response encoding, IPC, executor bookkeeping, and any preparation or startup
inside the selected envelope. It cannot isolate a universal orchestration cost.
Independent evaluation, result validation, provenance, durable publication, and
final command rendering remain outside the primary envelope. `time` measures the
whole command, including those operations.

## Measured local run

[measurements.json](measurements.json) retains all 198 repetitions, 18 scope and
setting summaries, failures, exact executable hashes, build receipts, host
observations, and the exported table hash. The run used Rust 1.98.0 release
builds on Linux with development environments and observed local controls.
Primary durations are medians; separate scope groups must stay separate.
The complete run command took 13.612 seconds, including evaluation, validation,
publication, and human output. Exports ran afterward and have separate command
timings in the retained report.

| Scope | No work, empty payload | 1 ms work, empty payload | 10 ms work, empty payload |
| --- | ---: | ---: | ---: |
| Execute only | 0.072 ms | 1.140 ms | 10.094 ms |
| Prepare and execute | 0.805 ms | 1.795 ms | 10.814 ms |
| Cold end to end | 11.169 ms | 10.538 ms | 20.868 ms |

For the empty no-work result, median envelope residuals were 0.046 ms,
0.779 ms, and 11.139 ms, respectively. Encoding a 1 MB payload took about
1.5–1.9 ms, with another 0.16–0.21 ms for writing. The execute-only primary
median for no work and a 1 MB payload was 2.000 ms. Small solver calls can be
dominated by these costs; choose a declared timing scope that answers the
scientific question and retain the worker diagnostics alongside it. The
measurements describe this host and run, not a portable performance guarantee.

The normal Rust suite checks that the controlled worker preserves timing
boundaries in all three scopes. It imposes no host-dependent performance limits.
