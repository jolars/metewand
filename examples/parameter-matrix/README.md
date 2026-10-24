# Parameter matrix

This benchmark generates integer arrays, sums them, and adds a problem-owned
offset. It keeps dataset `values`, problem `offset`, and implementation
`representation` in separate parameter namespaces. The implementation chooses
either a Python list or packed signed 64-bit integers; the evaluator independently
reads the canonical dataset and checks the mathematical answer.

The implementation declares `representation` once under `parameters`, with type
`string`, enum values `list` and `packed`, and default `list`. Metewand generates
its closed parameter schema and applies the literal default; no separate
implementation schema is needed. The experiment overrides that default with an
explicit grid of both representations. `plan` displays the resolved parameters
for each candidate. See [compact implementation parameters](../../docs/reference/parameter-namespaces.md#compact-implementation-parameters)
for the supported field declarations.

From the repository root, with Python 3 on `PATH`, inspect and run it:

```sh
cargo run --locked -p metewand-cli -- plan \
  --manifest examples/parameter-matrix/metewand.yaml --output json
cargo run --locked -p metewand-cli -- check --workers \
  --manifest examples/parameter-matrix/metewand.yaml
cargo run --locked -p metewand-cli -- run \
  --manifest examples/parameter-matrix/metewand.yaml \
  --output-dir "$(mktemp -d)/run" --output jsonl
```

The `cartesian` case crosses two dataset arrays with two offsets and two
representations, producing eight candidates. The `coupled` case fixes the array
to `[10, 10]` and the offset to 1, then inherits the two representations,
producing two more candidates. Cases are unioned, so the run has ten accepted
observations: answers 6, 8, 10, and 12 twice each, plus 21 twice. Every evaluator
metric is `absolute_error = 0`. Each candidate has one implementation repetition
and one measurement repetition.

The array entries use `grid` to select whole arrays in the first case and
`value` to preserve a literal array in the second. All grids expand
deterministically. Overlapping cases are errors rather than implicit statistical
replications; use repetition fields for that purpose. See
[configuration expansion](../../docs/reference/configuration-expansion.md).

The worker uses the raw protocol and Python's standard library. No SDK or
third-party package is needed, and all scientific choices remain visible in
this directory's contract, schemas, evaluator, and reference case.
