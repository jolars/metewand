# Dataset-free computation

This benchmark sums the integers from one through `n` for `n = 3` and `n = 10`.
The implementation uses an explicit sum, and the independent evaluator checks
`n * (n + 1) / 2`. The problem parameter schema bounds `n` to nonnegative
integers at most 10,000. These semantics belong to this example's contract.

From the repository root, with Python 3 on `PATH`, run:

```sh
cargo run --locked -p metewand-cli -- check --workers \
  --manifest examples/dataset-free/metewand.yaml
cargo run --locked -p metewand-cli -- run \
  --manifest examples/dataset-free/metewand.yaml \
  --output-dir "$(mktemp -d)/run" --output jsonl
```

The manifest omits `datasets`, and the contract accepts no dataset schemas.
Metewand supplies exactly one empty unit dataset per problem configuration.
Both workers assert that their private dataset directory is empty. The problem
reference case also omits a dataset and uses the default `n = 3`.

The run publishes two accepted observations with answers 6 and 55 and
`absolute_error = 0`. The raw command worker uses dedicated protocol pipes and
only the Python standard library. No SDK installation or data acquisition is
needed. See the [run guide](../../docs/reference/run-cli.md) for local execution guarantees.
