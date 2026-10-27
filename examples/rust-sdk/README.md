# Native Rust worker example

This benchmark sums `[1, 2, 3]`, adds the declared problem offset, and checks the
answer independently. Both workers are native Rust executables. Their callbacks
return result data or `Evaluation(metrics, verdict)`; Metewand's helpers handle
protocol messages and standard artifact envelopes.

From the repository root:

```sh
cargo build --locked -p metewand-runtime --example rust-worker
demo_dir=$(mktemp -d)
cp -R examples/rust-sdk/. "$demo_dir/"
cp target/debug/examples/rust-worker "$demo_dir/worker"
cargo run --locked -p metewand-cli -- check \
  --manifest "$demo_dir/metewand.yaml" --workers
cargo run --locked -p metewand-cli -- run \
  --manifest "$demo_dir/metewand.yaml" \
  --output-dir "$demo_dir/run" --output jsonl
```

The source bundle includes the compiled `worker` executable. Metewand stages
and hashes it, loads the fixed data, checks the reference fixture, and retains
two accepted observations with `{"answer":8}` and `{"absolute_error":0}`.
The native helper negotiates scientific rules and records `metewand-rust` SDK
metadata. The `prepare_and_execute` scope includes preparation, solving,
conversion, and result serialization. SDK `implementation_time_ns` covers only
the execute callback. Reset clears the prepared values between slots.

To inspect a scientific completion failure, change the implementation's `bias`
default from zero to one and run into a new output directory. The evaluator
returns `NotReached` with absolute error one. Its completion target still comes
from the problem declaration.

See the [Rust helper interfaces](../../crates/metewand-runtime/src/worker_sdk.rs)
for custom writers, errors, and lifecycle hooks, and
[extending a benchmark](../../docs/guide/authoring/extending.md#add-components)
for dependency setup in another Cargo project.
