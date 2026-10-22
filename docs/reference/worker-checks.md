# Worker checks

`metewand check --workers` extends the ordinary repository check by launching
trusted local workers. Use it before a benchmark run to verify protocol support
and exercise each problem's independent evaluator:

```sh
metewand check --workers
metewand check --manifest path/to/metewand.toml --workers --output json
```

The command resolves every declared generated-dataset materializer,
implementation, and problem evaluator before launching any worker. It checks
local executor requirements and negotiates each worker's protocol role and
content identity. Every declared implementation capability must appear in the
worker's hello response. The report labels selected capabilities `verified_now`
and also records additional reported capabilities without selecting them.
Fixed datasets have no worker to negotiate.

For every problem, the command evaluates all reference cases in contract order.
The evaluator receives the problem definition's `parameter_defaults`, or an
empty object when defaults are absent. These parameters must satisfy the
contract's parameter schema. Reference cases therefore need to describe that
configuration, independently of experiment parameter matrices.

Reference paths resolve from the repository root. A dataset reference may name
a directory containing `dataset-manifest.json` and its declared files, or a
single data file. For a single file or a directory without an inventory,
Metewand creates the same complete inventory used for fixed datasets. The
inventory must satisfy one of the contract's accepted dataset schemas.
Dataset-free cases omit `dataset` and receive an empty directory.

A result reference may name an artifact directory containing `result.json`, a
result envelope file, or a file containing only scientific result data. A raw
data file is wrapped in a version-1 result envelope with the contract's result
schema and no auxiliary files. Expected metrics similarly accept a metrics
envelope or raw metric data, either as a file or in a directory containing
`metrics.json`. Envelopes and scientific data must satisfy their public and
problem-owned schemas. References with auxiliary result files must use an
artifact directory.

The evaluator must return `reached` and write schema-valid metrics whose
canonical data exactly match the expected metrics. The command rejects
malformed artifacts, missing capabilities, protocol failures, nonmatching
metrics, and changes to staged reference inputs or resolved worker inputs.
Repository identities are checked again before reporting success.

All staging and evaluator outputs live in temporary directories. The command
does not execute implementation callbacks, materialize generated datasets,
fetch remote sources, install packages, create a cache, or publish a run.
Workers use the existing trusted local execution boundary, bounded logs,
phase deadlines, and process cleanup. Environments that require provisioning
fail explicitly, as they do for the current local run path.

Human, JSON, and JSON Lines output come from the same report. A successful
worker check adds `workers` and `reference_cases` to the ordinary check result,
and `worker_launches` counts negotiated workers. Each worker entry includes
its component, role, resolved identity, evidence, selected capabilities, and
reported capabilities. Each reference entry includes the problem name,
zero-based case index, and matching metrics. A failure emits the stable
`worker_check_failed` diagnostic and exits with status 3. Plain `check` retains
its side-effect-free behavior and emits neither worker evidence nor reference
execution results.

This adds an optional version-1 machine result shape and conformance vectors.
The public schema compatibility version remains 1 because existing check
results and envelopes remain valid. Manifest, identity, execution-semantics,
and wire-protocol compatibility versions are unchanged: the command uses their
existing contracts without changing their representations.
