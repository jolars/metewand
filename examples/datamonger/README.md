# Datamonger example

Resolve the pinned `2026.09` registry and `uci:iris@1` through Datamonger's Python
client, then compare two implementations using a shared Metewand dataset.
Install the optional client from a Datamonger checkout using its
[Python package guide](https://github.com/jolars/datamonger/blob/main/packages/python/README.md),
for example:

```sh
uv venv /tmp/metewand-datamonger
uv pip install --python /tmp/metewand-datamonger/bin/python \
  /path/to/datamonger/packages/python

cargo run --locked -p metewand-runtime --example datamonger -- \
  /tmp/metewand-datamonger/bin/python "$(command -v python3)" \
  /tmp/metewand-provider-cache /tmp/metewand-artifacts
```

The four arguments select the Datamonger interpreter, standard-library worker
interpreter, caller-owned provider cache, and Metewand artifact store. The last
two paths have existing parents and may be reused. On NixOS, use an interpreter
environment or wrapper that also supplies the client's native dependencies.
No Datamonger dependency is added to Metewand's default Rust build.

[`selector.json`](selector.json) records the complete registry selector, raw
index digest, dataset version, and required decoded verification. The Rust
driver passes these explicitly to the adapter; ambient Datamonger project
configuration cannot replace them. The provider process exports independent
raw files and reports verified decoding. Acquisition happens before dataset
construction and measurement.

The benchmark-owned materializer reads `artifacts/data`, selects the first CSV
column in source order, and converts the sepal lengths to exact integer tenths
using decimal arithmetic. It writes `data.json` and a standard, complete
`dataset-manifest.json`. Metewand validates the output schema and inventory,
publishes the dataset in its content-addressed store, and records the source
evidence, dataset seed, construction identity, and separate output hash.

The `reference` implementation prepares a Python list; `packed` prepares
little-endian signed 64-bit integers in memory. Both sum the same 150 values.
The expected answer is **8765**, with independent evaluator metrics
`{"absolute_error":0}`. The output prints one dataset-instance identity, two
distinct private view paths with the same tree digest, and the identical input
hash reported by both implementations. The views are temporary; the artifact
store retains the standard dataset and construction receipt. This lower-level
demonstration validates measurements without publishing durable attempt records.

After the first acquisition, append `--offline` to repeat from Datamonger's
verified cache without accessing upstream services. After resolution, the
materializer, implementations, and evaluator need only the local exports and
standard-library worker interpreter. They never use the provider cache or client.

Change `verification` to `artifact` in the selector to skip provider decoding.
Raw artifact verification still runs, and the materializer still performs the
benchmark-owned transformation. The evidence then says `not_verified` for
decoded values, and the construction identity changes. A request for `decoded`
always fails if decoded verification fails.

The manifest supplies the checked problem contract, schemas, parameters, and
planned seeds to the Rust driver. External selection remains a runtime API, so
run this example with the command above. `metewand check` and `metewand plan` can
inspect its manifest without acquiring data. The small `data.json` and
`reference/` files are contract reference cases, not a bundled Iris dataset.

Run the default offline example test with:

```sh
cargo test --locked -p metewand-runtime --example datamonger
```

Run the actual-client conformance test, using a private local registry, with:

```sh
METEWAND_DATAMONGER_PYTHON=/tmp/metewand-datamonger/bin/python \
  cargo test --locked -p metewand-runtime --example datamonger -- --ignored
```

See [sources and artifacts](../../CONTRIBUTING.md#sources-and-artifacts) for the
runtime boundary, the [adapter](../../crates/metewand-runtime/src/datamonger.rs)
for verification and failure handling, and [public schemas](../../schemas/v1)
for its request and response formats.
