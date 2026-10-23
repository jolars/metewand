# Motivating workflow smoke cases

These fixed, small cases begin the Gate 3 workflow ports on the Gate 2 one-shot
runner. Python 3 and its standard library suffice. Each workflow has a problem
contract, fixed input, canonical coefficients and intercept, expected metrics,
and an evaluator that imports no solver. The implementations are transparent
Python reference algorithms; later slices add the research package adapters and
Julia/R/Python interchange. These runs test scientific and execution contracts,
and do not compare package performance.

From the repository root:

```sh
python3 -B -m unittest discover -s examples/workflow-smoke -p test_science.py
cargo run --locked -p metewand-cli -- check \
  --manifest examples/workflow-smoke/metewand.yaml --workers
cargo run --locked -p metewand-cli -- plan \
  --manifest examples/workflow-smoke/metewand.yaml --output json
cargo run --locked -p metewand-cli -- run \
  --manifest examples/workflow-smoke/metewand.yaml \
  --output-dir /tmp/metewand-workflow-smoke --output json
```

Choose a fresh output directory. Expect three accepted observations, each with a
durable result, independently computed metrics, and provenance. Timing includes
preparation, execution, and result serialization. Startup and evaluation are
excluded. The environment is local and records observed controls. This example
does not provide locked publication dependencies.

## Extraction and expected results

[origins.json](origins.json) records the inspected source revisions, file hashes,
and local modification status. Inputs are committed here so running the smoke
cases does not require the originating repositories or their dependencies.
These are scientific reductions, not copies of the full study grids.

| Workflow | Source and reduction | Expected coefficients; intercept | Objective |
| --- | --- | --- | --- |
| Intercept strategies | `intercepts/test/gd.jl`, the six-row orthogonal Gaussian case, retaining its actual design and response and converting `reg=0.05` to canonical `lambda=1.92` | `[1.14, -0.5933333333333333, 0.16]`; `1.3` | `4.0256` |
| SLOPE | `benchmark_slope/objective.py` and the package paper's `bench_config_smoke_single.yml`; reduce the dense squared-loss comparison to a three-coordinate identity design with explicit decreasing canonical weights | `[1.65, 1.65, 0.25]`; `0` | `4.322083333333333` |
| Internal clustered lasso | `clustlasso-cd/experiments/general_design_check.py`, the actual `analytic_diagonal_both_penalties` case | `[-1, 0.5, 1]`; `0` | `1.190138888888889` |

The intercept objective is `||X beta + intercept - y||²/2 + lambda ||beta||₁`.
The intercept is unpenalized, and preprocessing preserves the supplied design.
The reference implementation eliminates the intercept by centering and uses
the orthogonal columns. Stationarity tests certify the analytic reference.
The original logistic prevalence grid and gradient/Newton/exact strategy
comparison remain later ports; this Gaussian case establishes coefficient,
intercept, and penalty conventions first.

The SLOPE objective is `||X beta - y||²/(2n) + sum(weights * sort(abs(beta)))`.
Its weights are already canonical scientific inputs, rather than package
regularization ratios or BH parameters. The implementation projects the shifted
sorted magnitudes with PAVA. The first two coordinates pool, so a worker that
only soft-thresholds coordinates fails the coefficient and objective checks.
The test certifies the tied subgradient against the rank weights.

Clustered lasso uses `||X beta - y||²/2 + l1 ||beta||₁ + fusion sum(i<j,
|beta_i-beta_j|)`, with each unordered pair counted once and no intercept.
Unequal diagonal curvatures expose accidental loss normalization. A bounded
proximal-gradient reference uses a sorted/PAVA prox; the evaluator independently
sums explicit pairs, and the tests certify stationarity. This fixture comes
from the internal algorithm comparison, separate from the external package
track.

The evaluator checks dimensions and finite coefficients, recomputes the full
objective, and compares coefficients, intercept, and objective with these
fixed analytic references at absolute tolerance `1e-8`. A finite inaccurate
result is scientifically `not_reached`; malformed dimensions or nonfinite
coefficients are invalid. The tests cover both outcomes and verify the committed
expected metrics. References are specific to these fixed inputs, not a generic
optimization oracle for arbitrary datasets.

## Acceptance slices still missing

| Workflow | Applicability cases to retain | Profile behavior to add |
| --- | --- | --- |
| Intercept strategies | Unsupported intercept strategies, sparse normalization, and unsafe dimensions must become structured per-cell decisions; supported cells must still execute. | A seeded Julia grid, shared preprocessing, independent target checks, and per-cell resume across strategies and prevalence settings. |
| SLOPE | Dense/sparse layout and intercept support vary by adapter. A declared package must not imply universal support. | Fresh iteration and tolerance sequences, streaming callbacks, evaluator-owned relative duality-gap targets, and elapsed-time alignment across heterogeneous controls. |
| Internal clustered lasso | Dimension and representation restrictions must preserve unsupported candidates and their reasons. | Direct traces, streaming callbacks, buffered local observers, diagnostic time, and independent target confirmation from shared inputs. |

The current manifest selects only supported dense one-shot cells. It does not
encode these decisions as runtime failures, manufacture convergence traces from
one endpoint, or treat solver controls as scientific budgets. No applicability
or profile capability is declared. Later roadmap slices extend these fixtures
with their first schema producers, consumers, SDK helpers, durable records,
inspection, and export support.

The Rust `local_examples` integration test runs the scientific acceptance tests,
`check --workers`, and all three one-shot executions, then checks coefficients,
metrics, durable artifact manifests, and recorded timing scope. Normal CI runs
this test. Existing versioned interfaces suffice for these author-owned
contracts; this slice changes no compatibility constants or version-1 bytes.
