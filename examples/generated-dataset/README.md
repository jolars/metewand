# Generated dataset example

Run a benchmark-owned generator followed by an implementation and independent
evaluator, using Python's standard library and the raw worker protocol:

```sh
cargo run --locked -p metewand-cli -- run \
  --manifest examples/generated-dataset/metewand.toml \
  --output-dir /tmp/metewand-generated --output jsonl
```

The generator receives the planned dataset seed and canonical parameters. It
writes three integers and a complete dataset manifest. The implementation sums
them and adds two; the expected answer is eight. Metewand validates the manifest,
records the construction identity and separate output hash in attempt
provenance, and materializes each dataset configuration once per run.

A dataset's optional `source` table may supply a pinned HTTP(S) JSON input with a
`values` array. In that case this example transforms the verified raw file named
`source` instead of generating the parameter values. See
[dataset materialization](../../docs/dataset-materialization.md) for the runtime
API, external-provider evidence, and contract-owned binary validation.
