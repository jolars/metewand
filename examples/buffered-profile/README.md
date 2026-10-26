# Buffered profiles

Run one halving trajectory against the objective `x²`, with an independent
evaluator. Python direct recording and local observers share one artifact
contract with the native Rust worker. Three thresholds select three snapshots
from each target trajectory. A second experiment collects a fixed count.

Stage the example and the in-tree SDK from the repository root:

```sh
benchmark_dir="$(mktemp -d)"
cp -R examples/buffered-profile/. "$benchmark_dir/"
cp -R sdk/python/src/metewand "$benchmark_dir/metewand"
cargo run --locked -p metewand-cli -- check --manifest "$benchmark_dir/metewand.yaml" --workers
cargo run --locked -p metewand-cli -- plan --manifest "$benchmark_dir/metewand.yaml" --output json
cargo run --locked -p metewand-cli -- run --manifest "$benchmark_dir/metewand.yaml" --output-dir "$benchmark_dir/run" --output json
cargo run --locked -p metewand-cli -- status --output-dir "$benchmark_dir/run"
cargo run --locked -p metewand-cli -- run --manifest "$benchmark_dir/metewand.yaml" --output-dir "$benchmark_dir/run" --resume
cargo run --locked -p metewand-cli -- export --output-dir "$benchmark_dir/run" --destination "$benchmark_dir/jsonl" --format jsonl
cargo run --locked -p metewand-cli -- export --output-dir "$benchmark_dir/run" --destination "$benchmark_dir/csv" --format csv
```

Expect five accepted attempts and fifteen independently evaluated snapshots.
Warm-ups use the same buffered path but produce no measured observations.
Unsaved checks appear as diagnostic trace rows in exports. Compare curves using
their producing-attempt identities and instrumented time coordinates.

The evaluator deliberately returns the one-shot completion verdict
`not_reached` for every nonzero iterate. The profile instead uses its declared
metric threshold, demonstrating that scientific validity and profile completion
are separate decisions.

For the native loop, build and stage the Rust worker:

```sh
cargo build --locked -p metewand-runtime --example buffered-worker
cp target/debug/examples/buffered-worker "$benchmark_dir/native-worker"
cargo run --locked -p metewand-cli -- check --manifest "$benchmark_dir/native.yaml" --workers
cargo run --locked -p metewand-cli -- run --manifest "$benchmark_dir/native.yaml" --output-dir "$benchmark_dir/native-run"
```

The native worker performs every local check in Rust and uses Python only for
the independent evaluator process. These are dependency-free regression
examples, not performance comparisons of Basin or clustlasso-cd.

See [buffered profiles](../../docs/reference/buffered-profiles.md) for policy,
clock, collection, independent evaluation, and retry rules. An interrupted run
retains already published snapshots. Use `--resume --retry` to authorize a new
complete trajectory for an unsatisfied slot.
