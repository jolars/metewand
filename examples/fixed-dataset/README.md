# Fixed local dataset

From the repository root, with Python 3 available on `PATH`, run:

```sh
cargo run --locked -p metewand-cli -- run \
  --manifest examples/fixed-dataset/metewand.yaml \
  --output-dir /tmp/metewand-example \
  --output jsonl
```

Choose a new output directory for every invocation. This example uses the raw
version-1 worker protocol over dedicated pipes, with no package or third-party
Python dependencies. The implementation returns `{"answer":8}`; the independent
evaluator returns `{"absolute_error":0}`. Progress events and the final report
are versioned JSON Lines. Worker standard streams are retained as scoped logs
inside the finalized attempt directory.

The output contains one observation directory and one enclosing attempt
directory, each with a hash-complete `manifest.json`. See the
[run command documentation](../../docs/guide/authoring/running.md) for supported policies,
source-reference arguments, failure behavior, and output schemas.

The implementation omits its parameter schema, so it accepts only `{}`.
Worker environments may also be omitted: with no environment declarations,
Metewand supplies a local environment. Nested policy versions and both repetition
counts default to `1`. The manifest keeps those fields visible here for easy
experimentation. Timing scope, primary clock, and the one-shot profile remain
explicit. Inspect all effective settings with `metewand plan --output json`.
