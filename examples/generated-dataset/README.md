# Generated dataset example

Run a benchmark-owned generator followed by two implementations and an independent
evaluator, using Python's standard library and the raw worker protocol:

```sh
cargo run --locked -p metewand-cli -- run \
  --manifest examples/generated-dataset/metewand.yaml \
  --output-dir /tmp/metewand-generated --output jsonl
```

The generator receives the planned dataset seed and canonical parameters. It
writes three signed 64-bit integers and a complete dataset manifest. The
`reference` implementation loads the JSON values into a Python list during
`prepare`; `packed` converts them into little-endian signed 64-bit bytes in
memory. Both preserve the original values and order. During `execute`, each
implementation sums its prepared values and adds two; both produce eight.
Metewand validates the manifest,
records the construction identity and separate output hash in attempt
provenance, and materializes each dataset configuration once per run. Each
attempt receives a private copy of that instance. Add
`--cache-dir /tmp/metewand-artifacts` and choose a new output directory on each
invocation to reuse the verified construction across runs. See
[sources and artifacts](../../CONTRIBUTING.md#sources-and-artifacts) for cache
ownership and retention.

The contract's `semantics.preprocessing` rules describe shared construction and
implementation conversion, and prohibit untimed implementation preprocessing.
The selected `prepare_and_execute` policy includes JSON loading, packing, and
canonical result writing. Shared dataset construction runs once before either
implementation is measured. Neither implementation writes to its dataset view.
The [conversion vector](fixtures/preparation.json) and
[conformance tests](../../crates/metewand-cli/tests/dataset_preparation.rs)
check these rules across repeated attempts. See
[materialization and implementation preparation](../../docs/guide/authoring/running.md#choose-what-to-time)
for scope selection and the distinction between semantic rules and runtime
enforcement.

A dataset's optional `source` table may supply a pinned HTTP(S) JSON input with a
`values` array. In that case this example transforms the verified raw file named
`source` instead of generating the parameter values. See
[dataset materialization](../../CONTRIBUTING.md#sources-and-artifacts) for the runtime
overview and external-provider boundary. The
[materialization module](../../crates/metewand-runtime/src/dataset_materialization.rs)
defines the runtime API and optional binary validation.
