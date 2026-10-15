# Fixed local dataset

From the repository root, with Python 3 available on `PATH`, run:

```sh
cargo run --locked -p metewand-cli -- run \
  --manifest examples/fixed-dataset/metewand.toml \
  --output-dir /tmp/metewand-example \
  --output jsonl
```

Choose a new output directory for every invocation. This example uses the raw
version-1 worker protocol over dedicated pipes, with no SDK or third-party
Python dependencies. The implementation returns `{"answer":8}`; the independent
evaluator returns `{"absolute_error":0}`. Progress events and the final report
are versioned JSON Lines. Worker standard streams are retained as scoped logs
inside the finalized attempt directory.

The output contains one observation directory and one enclosing attempt
directory, each with a hash-complete `manifest.json`. See the
[run command documentation](../../docs/run-cli.md) for supported policies,
source-reference arguments, failure behavior, and output schemas.
