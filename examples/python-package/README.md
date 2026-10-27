# Python package example

This benchmark generates `[1, 2, 3]`, sums the values and adds two, then
evaluates the answer independently. It exercises the three package worker roles, two
measured slots, process reuse, and durable result publication.

The problem's parameters, results, metrics, scientific rules, reference cases,
and evaluator live directly under `problems.sum` in `metewand.yaml`, with no
`contract` wrapper or separate family semantics schema. Result and metric shapes
use compact declarations, as do dataset parameters. The manifest relies on
unambiguous local-environment and one-shot defaults, keeping timing and
scientific choices explicit. No separate contract or schema files are needed.
The dataset schema uses a YAML anchor so the dataset and problem share one
declaration. The adapters return result data and an explicit metric verdict;
the package writes the envelopes, and Metewand validates them against the selected
problem contract.

From the repository root, copy the example and package into a temporary benchmark
directory and run it:

```sh
demo_dir=$(mktemp -d)
cp -R examples/python-package/. "$demo_dir/"
cp -R packages/python/src/metewand "$demo_dir/metewand"
cargo run --locked -p metewand-cli -- run \
  --manifest "$demo_dir/metewand.yaml" \
  --output-dir "$demo_dir/run" --output jsonl
```

Start a benchmark with the same structure using `metewand init`, then
`metewand scaffold dataset|problem|implementation NAME --language python`
(with `--problem NAME` for implementations). Fill the scientific TODOs and
reference fixtures, then add experiments and policies. See the
[authoring guide](../../docs/guide/authoring/index.md) for all three languages.

The explicit source bundle includes `worker.py` and the package's Python files, so
Metewand hashes and stages them together. Python imports the staged package
beside the worker without installation or an ambient `PYTHONPATH`. Python 3.11
or later must be on `PATH`. The package has no runtime dependencies.

Both observations contain `{"answer":8}` and evaluator metrics
`{"absolute_error":0}`. Dataset construction happens once. `prepare` loads the
private dataset view, `execute` returns the answer, and the package writes
the completed result envelope. The `prepare_and_execute` scope includes all
three operations; the secondary package `implementation_time_ns` covers only the
`execute` callback. The runtime resets the implementation between slots.

The evaluator computes the expected answer from its private dataset view and
supplied problem parameters. `serve_evaluator(..., semantics=True)` passes the
contract rules to the evaluator, which reads its completion tolerance from
`one_shot_completion.absolute_error`. The package handles protocol messages and error
responses; the runtime owns scheduling, validation, and publication. See the
[Python callbacks](../../packages/python/src/metewand/_worker.py) and
[artifact helpers](../../packages/python/src/metewand/_artifacts.py) for their interfaces.
