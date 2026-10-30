# Parameter matrix

This benchmark generates integer arrays, sums them, and adds a problem-owned
offset. It keeps dataset `values`, problem `offset`, and implementation
`representation` in separate parameter namespaces. The implementation chooses
either a Python list or packed signed 64-bit integers; the evaluator independently
reads the canonical dataset and checks the mathematical answer.

The dataset declares `values` once under `parameters`, with an array type,
integer items, and a default grid of two whole arrays. The contract declares
`offset` with a literal default of 2, plus compact result and metric fields.
The implementation declares `representation` with type `string`, enum values
`list` and `packed`, and default `list`. Metewand generates closed schemas with
required fields from these declarations. The `schemas/` directory retains
explicit equivalents for reuse, alongside the dataset and semantics schemas
used by the benchmark.

The experiment overrides the representation default with a grid of both
representations. `plan` displays the resolved parameters for each candidate.
See [compact declarations](../../docs/guide/authoring/parameters.md)
for the supported fields and selection precedence. Result and metric envelopes
use the selected contract's schemas without repeating schema paths in worker
code.

From the repository root, with Python 3 on `PATH`, inspect and run it:

```sh
cargo run --locked -p metewand -- plan \
  --manifest examples/parameter-matrix/metewand.yaml --output json
cargo run --locked -p metewand -- check --workers \
  --manifest examples/parameter-matrix/metewand.yaml
cargo run --locked -p metewand -- run \
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

The `cartesian` case inherits the dataset declaration's grid. The `coupled`
case overrides it with `value` to preserve a single literal array. It also
uses the plain scalar `offset: 1`, which normalizes to the same axis
as `offset: {value: 1}`. Arrays and objects require an explicit `value` or `grid`
wrapper. The grid selects runs without restricting the valid array domain.
All grids expand deterministically. Overlapping cases are errors rather than implicit statistical
replications; use repetition fields for that purpose. See
[configuration expansion](../../CONTRIBUTING.md#manifests-and-planning).

The worker uses the raw protocol and Python's standard library. No package or
third-party package is needed, and all scientific choices remain visible in
this directory's contract, schemas, evaluator, and reference case.
