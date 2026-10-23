# Python SDK example

This benchmark generates `[1, 2, 3]`, sums the values and adds two, then
evaluates the answer independently. It exercises the three SDK worker roles, two
measured slots, process reuse, and durable result publication.

The problem contract and every scientific schema are inline in `metewand.yaml`.
The dataset schema uses a YAML anchor so the dataset and problem share one
declaration. The worker writes result and metric envelopes without schema paths;
Metewand validates them against the selected problem contract.

From the repository root, copy the example and SDK into a temporary benchmark
directory and run it:

```sh
demo_dir=$(mktemp -d)
cp -R examples/python-sdk/. "$demo_dir/"
cp -R sdk/python/src/metewand "$demo_dir/metewand"
cargo run --locked -p metewand-cli -- run \
  --manifest "$demo_dir/metewand.yaml" \
  --output-dir "$demo_dir/run" --output jsonl
```

The explicit source bundle includes `worker.py` and the SDK's Python files, so
Metewand hashes and stages them together. Python imports the staged package
beside the worker without installation or an ambient `PYTHONPATH`. Python 3.11
or later must be on `PATH`. The SDK has no runtime dependencies.

Both observations contain `{"answer":8}` and evaluator metrics
`{"absolute_error":0}`. Dataset construction happens once. `prepare` loads the
private dataset view, `execute` computes the answer, and `write_result` writes
the completed result envelope. The `prepare_and_execute` scope includes all
three operations; the secondary SDK `implementation_time_ns` covers only the
`execute` callback. The runtime resets the implementation between slots.

The evaluator computes the expected answer from its private dataset view and
supplied problem parameters. The SDK handles protocol messages and error
responses; the runtime owns scheduling, validation, and publication. See the
[Python SDK guide](../../docs/reference/python-sdk.md) for callback signatures and output
helpers.
