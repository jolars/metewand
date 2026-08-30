# Metewand: Reproducible, Language-Neutral Optimization Benchmarks

## 1. Purpose

`metewand` benchmarks numerical optimization **software**, not merely algorithms. A benchmark may compare R packages, Python packages, Julia packages, native executables, or any mixture of them without privileging one host language.

The central object is an immutable experiment composed of:

```text
configured dataset + configured problem + configured solver
    + software environment + execution policy
```

Datasets, problems, and solvers are parameterized definitions rather than fixed objects. Expanding an experiment produces immutable dataset instances, problem instances, solver configurations, and, ultimately, run specifications.

The tool must make simple, single-language work pleasant while supporting publication-grade runs with pinned data, pinned environments, controlled execution, and complete provenance.

### Design principles

1. **One language-neutral orchestrator.** The main product is a standalone binary; it never embeds R, Python, or Julia.
2. **Processes, not FFI.** Workers communicate through a versioned protocol over pipes. No `rpy2`, PythonCall, RCall, or embedded interpreters.
3. **Native workflows remain native.** An R-only benchmark may use `renv`; a Python-only benchmark may use `uv`; a Nix-based benchmark may provide any or all worker environments from flakes. None requires another environment backend.
4. **Reproducibility is explicit.** Datasets, source files, lockfiles, images, and configurations are hashed or pinned.
5. **Parameter namespaces are explicit.** Dataset, problem, solver, budget, and execution parameters are never conflated.
6. **Evaluation is independent of solvers.** Solvers return canonical results; a benchmark-owned evaluator computes objectives, gaps, feasibility, and other metrics.
7. **Strictness is graduated.** Local exploratory, locked, and isolated execution modes make distinct, limited guarantees.
8. **Automation is a first-class interface.** The CLI is deterministic, noninteractive, inspectable, and machine-readable so the same workflows compose cleanly in shells, CI, and agent loops.

## 2. User model

A repository contains a manifest, optional native lockfiles, dataset materializers, solver/evaluator adapters, schemas, and a generated lockfile:

```text
benchmark/
├── metewand.toml
├── metewand.lock
├── flake.nix                 # optional Nix environments
├── flake.lock
├── datasets/
│   └── leukemia.py
├── evaluators/
│   └── lasso.py
├── solvers/
│   ├── glmnet.R
│   ├── sklearn.py
│   └── native.toml
├── r/
│   ├── renv.lock
│   └── renv/
├── python/
│   ├── pyproject.toml
│   └── uv.lock
├── schemas/
│   ├── lasso-problem-parameters.json
│   ├── lasso-metrics.json
│   ├── lasso-result.json
│   ├── leukemia-dataset-parameters.json
│   ├── regression-dataset.json
│   ├── glmnet-parameters.json
│   └── sklearn-parameters.json
└── data/                    # optional local, hashed artifacts
```

A representative manifest is:

```toml
version = 1
name = "lasso-comparison"

[problems.lasso]
contract_version = 1
parameter_schema = "schemas/lasso-problem-parameters.json"
dataset_schema = "schemas/regression-dataset.json"
result_schema = "schemas/lasso-result.json"
metric_schema = "schemas/lasso-metrics.json"
evaluator = "lasso"

[evaluators.lasso]
runner = "python"
entrypoint = "evaluators/lasso.py"
environment = "python"

[datasets.leukemia]
parameter_schema = "schemas/leukemia-dataset-parameters.json"
output_schema = "schemas/regression-dataset.json"
runner = "python"
entrypoint = "datasets/leukemia.py"
environment = "python"

[datasets.leukemia.source]
url = "https://example.org/leukemia.tar.zst"
sha256 = "..."

[environments.r]
kind = "renv"
project = "r"
lockfile = "r/renv.lock"

[environments.python]
kind = "uv"
project = "python"
lockfile = "python/uv.lock"

[environments.native]
kind = "nix"
flake = "."
installable = ".#native-solver"
system = "x86_64-linux"

[solvers.glmnet]
runner = "r"
entrypoint = "solvers/glmnet.R"
environment = "r"
parameter_schema = "schemas/glmnet-parameters.json"

[solvers.sklearn]
runner = "python"
entrypoint = "solvers/sklearn.py"
environment = "python"
parameter_schema = "schemas/sklearn-parameters.json"

[solvers.native]
runner = "command"
command = ["native-solver", "--metewand-worker"]
environment = "native"

[[experiments]]
name = "main"
problem = "lasso"
datasets = ["leukemia"]
solvers = ["glmnet", "sklearn", "native"]
execution_policy = "controlled"
repetitions = 5
seed = 2025

[experiments.dataset_parameters.leukemia]
fold = { grid = [1, 2, 3] }
standardize = { value = true }

[experiments.problem_parameters]
lambda = { grid = [0.001, 0.01, 0.1] }
fit_intercept = { value = true }

[experiments.solver_parameters.glmnet]
tolerance = { value = 1e-7 }

[experiments.solver_parameters.sklearn]
selection = { grid = ["cyclic", "random"] }

[execution_policies.controlled]
version = 1
cpus = 1
threads = 1
memory = "8 GiB"
network = false
worker_reuse = false
warmup_runs = 1
primary_time = "solver_time"
run_order = "randomized"
enforcement = "best_effort"
```

Primary commands:

```text
metewand check                 validate manifest, schemas, and workers
metewand schema                print versioned public schemas
metewand plan                  show the expanded, side-effect-free run plan
metewand lock                  resolve artifacts and write metewand.lock
metewand run                   exploratory run; record what was used
metewand run --locked          refuse any lock or artifact mismatch
metewand status                inspect attempts and resumable state
metewand explain <id>          explain a specification, artifact, or failure
metewand export --format csv   export tidy results
```

Each parameter entry is either a literal `value` or an expansion `grid`. This distinction permits arrays and tables to be literal parameter values. Grids form a Cartesian product across the dataset, problem, and applicable solver namespaces. In version 1, selected non-Cartesian combinations are expressed as separate named experiment entries; zipped axes may be added later. Parameter values use the JSON data model with finite numbers. After schema validation and explicit default resolution, they are serialized as canonical JSON for hashing and transport. Expansion order and derived seeds are deterministic.

Parameter ownership is semantic rather than inherited from a library API:

- dataset parameters determine acquired, generated, selected, or transformed data, such as sample size, noise, preprocessing, or a fold;
- problem parameters determine the mathematical problem, such as the loss, regularization strength, intercept convention, or constraints;
- solver parameters determine how one implementation solves that problem, such as tolerance, algorithm variant, or internal initialization;
- budgets and execution parameters determine the comparison conditions without changing the problem or solver configuration.

Every selected solver configuration must solve the same problem instance. A library option that changes the objective, constraints, data transformation, or result interpretation is therefore a dataset or problem parameter even if the library presents it as a solver option. Metewand validates declared schemas and namespaces, but semantic classification remains the benchmark author's responsibility and is reviewable in the problem contract.

Schema identifiers and contents are versioned and hashed. Before materialization or execution, the planner requires each dataset definition's output schema to match a schema accepted by the selected problem contract. Cross-schema conversion requires an explicit dataset materializer; it is never inferred from field names or shapes.

The experiment seed deterministically derives separately recorded dataset, problem, solver, and scheduling seeds. By default, `repetitions` varies only the solver seed and repeats a solver configuration against the same problem instance. Dataset or problem replication is expressed through an explicit parameter axis such as `replicate`; that value participates in the corresponding derived seed.

## 3. Core architecture

The implementation is a Rust workspace with one distributable executable and small optional language SDKs:

```text
metewand/
├── crates/
│   ├── metewand-cli/          CLI and user-facing diagnostics
│   ├── metewand-core/         manifest, planning, scheduling, run records
│   ├── metewand-protocol/     wire types and protocol versioning
│   └── metewand-runtime/      artifacts, environments, executors, provenance
├── sdk/
│   ├── r/
│   ├── python/
│   └── julia/                 after the initial release
├── schemas/
├── examples/
│   ├── r-only/
│   ├── python-only/
│   ├── nix-locked/
│   └── mixed/
└── docs/
```

The binary owns:

- manifest parsing and experiment expansion;
- dataset acquisition, materialization, and verification;
- lockfile generation and checking;
- environment provisioning and fingerprinting;
- worker lifecycle and scheduling;
- resource settings and outer-process measurement;
- evaluator invocation;
- provenance and result storage.

Language SDKs own only:

- protocol serialization;
- dispatch to user-supplied callbacks/classes;
- consistent error reporting;
- monotonic timing around the solver call.

Any executable may implement the protocol directly; an SDK is never required.

### Automation and agentic workflows

The command-line interface is the universal automation API. It must work without a terminal, browser, editor integration, or model-specific plugin. Claude, Codex, other agents, CI jobs, and ordinary scripts receive the same semantics; an optional MCP adapter may later expose these operations without creating a second behavioral API.

Every command supports a versioned machine-readable output mode. In that mode, standard output contains only JSON or JSON Lines conforming to published schemas; progress and human diagnostics go to standard error. Exit codes and diagnostic codes are stable and documented. Structured diagnostics include source spans, affected stable identities, causal chains, and concrete remediation where it can be stated safely. Human-readable output is rendered from the same typed records rather than maintained as a separate source of truth.

Commands are noninteractive by default and never silently rewrite manifests, native lockfiles, or source files. An operation that requires an additional mutation or capability fails with an explanation and the explicit flag or command that authorizes it. Destructive cleanup is a separate command with a dry-run mode and exact artifact targets.

The read-only `plan` command performs schema validation, deterministic expansion, compatibility checks, and logical run-ID assignment without downloads, builds, worker launches, or filesystem mutation. Resolution through `lock` adds artifact and environment fingerprints and produces a separate resolved execution ID. This distinction keeps plans inspectable before expensive provisioning without pretending that an unresolved environment is known. Its output includes:

- every dataset, problem, solver, and logical run specification identity;
- the number and order of planned attempts;
- required artifacts, environment resolutions, executors, and controls;
- anticipated network access, builds, commands, and writable paths;
- unsupported capabilities and blockers known before execution.

Long-running operations emit append-only structured events and journal state atomically. `run --resume` continues from finalized artifacts and terminal attempt states without repeating successful work; interrupted staging directories are diagnosed and either safely resumed or quarantined. Filters operate on stable logical or resolved identities, and explicit bounds such as maximum attempts or selected run IDs let an agent test a small change before expanding to the complete benchmark.

This interface is agent-friendly, not agent-dependent. Metewand does not embed a model, send source or results to an external service, generate scientific conclusions, or grant an agent authority beyond the invoked command. All operations remain usable and auditable as ordinary local CLI calls.

### Main domain types

```text
Benchmark             named collection of experiments
DatasetDefinition     source or generator, materializer, and parameter schema
DatasetConfiguration  definition + canonical parameters + dataset seed
DatasetInstance       immutable artifact materialized from a dataset configuration
ProblemDefinition     semantic contract, parameter schema, result schema, and evaluator
ProblemConfiguration  definition + canonical parameters + optional problem seed
ProblemInstance       dataset instance + problem configuration
SolverDefinition      adapter, parameter schema, and capabilities
SolverConfiguration   definition + canonical parameters + selected environment
Evaluator             benchmark-owned computation of trusted metrics
Environment           reproducible software context for a worker
Executor              mechanism that launches and constrains a worker
ExecutionPolicy       resources, isolation, timing policy, and repetition policy
LogicalRunSpecification
                      dataset + problem + solver configurations + budget + policy + solver seed
ResolvedRunSpecification
                      logical specification + materialized instance + resolved artifacts/environments
RunAttempt            one observed execution of a resolved run specification
```

Definitions describe parameterized families, and configurations bind canonical parameter values. Logical specifications can therefore be identified before acquisition or environment provisioning. Resolution binds immutable artifacts and environment fingerprints into a second identity containing every declared input needed for execution. A `RunAttempt` adds observed controls, machine context, state, timestamps, and results without changing either identity.

`SolverDefinition` and `Environment` remain deliberately separate. The same adapter may run locally during development and in an OCI image for an archival run.

### Problem and fairness contracts

A problem definition is a versioned, executable contract rather than a name. It binds:

- accepted dataset and problem-parameter schemas;
- canonical result and metric schemas;
- the evaluator and its environment;
- objective, constraint, and intercept/scaling conventions;
- supported budget types and their semantics;
- correctness tolerances and rules for invalid results.

Every experiment also selects a versioned execution policy. The policy specifies cold or warm execution, worker reuse, permitted preprocessing and caching, warm-up runs, whether compilation is timed, run ordering and randomization, timeouts, resource requirements, and which timing field is authoritative. Adapters must not move work across timing boundaries in a way that violates the policy. This contract makes runs scientifically comparable; the evaluator makes their outputs independently checkable.

Iteration counts are not presumed comparable across implementations. Evaluation, iteration, and time budgets are permitted only when the problem contract defines their meaning and the worker declares the corresponding capability. Otherwise, the benchmark compares final results under explicit solver stopping rules.

## 4. Worker protocol

Use a versioned JSON-Lines protocol over two dedicated pipes. Standard output and standard error are captured as worker logs, so unexpected output from a numerical library cannot corrupt the protocol. SDKs receive the protocol handles from the orchestrator; raw command workers may explicitly request a stdin/stdout compatibility mode. Large arrays and datasets are passed as paths to read-only artifact directories, never embedded in JSON.

Every request and response carries a request ID. Version negotiation, role, and capabilities are established before any work begins. Version 1 permits at most one outstanding request per worker, but IDs make timeouts, late responses, and diagnostics unambiguous.

The orchestrator begins a worker session with a handshake:

```json
{"id":"0","method":"hello","protocols":[1],"role":"solver"}
```

The worker responds with supported capabilities, such as:

```json
{"id":"0","ok":true,"protocol":1,"capabilities":["one_shot","iteration_budget"]}
```

Dataset materializers support:

```text
materialize(source_dir, dataset_parameters, dataset_seed, output_dir)
shutdown()
```

The materializer writes a canonical dataset manifest and files beneath `output_dir`. A dataset definition without a materializer treats its verified source tree as the dataset instance and therefore cannot accept materialization parameters.

Mandatory solver operations:

```text
prepare(instance_dir, problem_parameters, solver_parameters, solver_seed)
solve(result_dir, optional_budget)
reset()
shutdown()
```

`solve` returns scalar metadata and a manifest of files under `result_dir`:

```json
{
  "id": "2",
  "ok": true,
  "solver_time_ns": 18342011,
  "result": {"manifest": "result.json"},
  "statistics": {"iterations": 37}
}
```

The canonical result schema belongs to the problem contract. For a lasso problem it might require `coefficients` and `intercept`, regardless of how individual libraries represent them.

Evaluator workers support:

```text
evaluate(instance_dir, problem_parameters, result_dir, metrics_path)
```

They compute objective values, duality gaps, feasibility, prediction metrics, or correctness checks. Solver-reported metric values may be stored as diagnostics but are not authoritative.

Workers are persistent by default so interpreter startup and package loading are measured separately from solving. `worker_reuse = false` provides strict process isolation. The orchestrator calls `reset` between attempts, and every `prepare` must establish clean state even when a worker is reused. A worker that times out, crashes, violates the protocol, or fails to reset is discarded.

A run attempt advances through `planned`, `materializing`, `preparing`, `solving`, `evaluating`, and `finalizing` states. Success and each typed failure are terminal. State transitions and their timestamps are journaled so interruption cannot leave an apparently successful partial run.

The orchestrator validates every referenced path and output manifest, rejects path traversal and undeclared files, and never trusts a worker-supplied path outside its assigned directories. Dataset and result inputs are mounted or opened read-only where the executor supports it; each operation receives a private output directory.

Errors have stable machine-readable codes, a human-readable message, and optional structured details. The orchestrator distinguishes at least invalid configuration, unsupported capability, materialization failure, setup failure, timeout, resource-limit violation, worker crash, malformed protocol, invalid result, evaluator failure, and internal error. A run attempt records one of these outcomes rather than disappearing because it did not produce metrics. A timeout initiates an executor-defined termination sequence—interrupt, bounded grace period, then forced process-tree termination. Partial outputs are retained only as diagnostic artifacts and never accepted as canonical results.

The first release requires one-shot solves and may support repeated fresh runs at problem-defined iteration, evaluation, or time budgets. Streaming checkpoints and callback-based trajectories are later protocol extensions.

The protocol and executor are not a security boundary for hostile benchmark code. Benchmark repositories and workers are trusted inputs; isolation controls protect reproducibility and limit accidental interference. Running untrusted submissions requires a separately designed sandbox or an integration such as BenchExec.

## 5. Reproducibility model

`metewand.lock` is a **lockfile of definitions, configurations, artifacts, and native lockfiles**. It does not replace `renv.lock`, `uv.lock`, `flake.lock`, Julia `Manifest.toml`, or OCI manifests; it records and binds their identities into the benchmark.

It must contain at least:

- normalized manifest and execution-policy hashes;
- hashes of dataset materializers, problem contracts, schemas, evaluators, and solver adapters;
- canonical dataset, problem, and solver configuration identities;
- dataset source hashes, materialization recipes, and output-tree hashes;
- hashes of native environment lockfiles;
- resolved runtime and package-manager requirements and fingerprints;
- for Nix environments, the evaluated flake source and lock identities, installable, target system, derivation, realized outputs, and the digest of a recursive closure manifest;
- OCI image digests; tags alone are invalid in locked mode;
- tool and wire-protocol versions;
- generator inputs, parameters, seeds, environment identity, and expected output hash for generated datasets.

Artifact resolution may use the network. Actual benchmark runs default to no network access.

Four environment-resolution classes are supported:

| Mode | Environment | Guarantee |
|---|---|---|
| Development | Existing local runtime | Observed provenance only; convenient, but not reliably recreatable |
| Ecosystem-locked | `uv`, `renv`, later Julia `Pkg` | Declared dependencies and inputs must match their locks; undeclared system and platform dependencies remain recorded limitations |
| Closure-locked | Nix flake installable | Evaluated source, derivation, realized store outputs, and dependency closure are identified; runtime kernel and hardware remain external |
| Image-pinned | OCI image by digest | Immutable declared userspace image; host kernel, hardware, runtime, and explicitly mounted inputs remain external |

Environment resolution and execution isolation are independent. A digest-pinned image does not by itself prove that networking, memory, CPU use, or filesystem writes were constrained. Each executor advertises its controls, and each run records every control as requested, enforced, best-effort, or unsupported. With `enforcement = "required"`, planning fails before execution if the selected executor cannot enforce a requested control. Best-effort execution remains available for exploratory runs but is never presented as equivalent to enforced execution.

The tool must not claim bitwise-identical results or identical performance across machines. Every run records hardware and operating context, including CPU model, architecture, selected cores, memory, kernel, container runtime, language runtime, BLAS implementation, relevant thread variables, tool version, repository commit, and dirty-tree hash. Where available, it also records CPU governor, frequency/turbo state, NUMA placement, accelerator identity, and background-load indicators. Missing provenance is represented explicitly rather than silently omitted.

Timing fields are distinct:

```text
materialization_time  dataset generation or transformation
worker_startup_time   interpreter and package loading
setup_time            permitted conversion/preprocessing outside the solve call
solver_time           monotonic in-worker timing around the solver call
wall_time             executor-observed solve-request duration
evaluation_time       independent result evaluation
cpu_time / peak_rss   executor-observed process-tree use where supported
```

The execution policy identifies the primary timing measure. Measurements that cannot include the complete worker process tree are marked partial. Warm-up and untimed preparation are recorded even when excluded from the primary measure.

## 6. Environments, execution, and artifacts

Environment backends produce a resolved worker launch specification and a fingerprint:

```text
local     use an existing executable; provenance only
uv        create/sync a Python project with its lockfile
renv      restore an R project with its lockfile
nix       build a flake installable and launch from its store output
oci       execute an image pinned by digest
```

Julia `Pkg` is a planned addition. Conda is not a privileged dependency and need not be supported initially.

### Nix environments

Nix is a first-class locked environment backend. A Nix environment names a flake reference, an installable, and a target system. It may provide an environment for a dataset materializer, evaluator, solver, or later analysis worker. The worker definition still owns its entrypoint or command; the resolved Nix output supplies the executable and runtime environment. Standard flake installables are the integration boundary, and Metewand does not require a separate `devenv` backend.

Resolution builds the installable before any benchmark timing and returns a launch specification rooted in the realized store output. Workers are launched directly from that output; `nix run`, `nix develop`, evaluation, substitution, and build time are never included in worker startup or solve timing.

In locked mode, Metewand forbids impure evaluation and refuses any operation that would create, update, or rewrite `flake.lock`. The environment fingerprint records:

- the exact evaluated flake source-tree hash and `flake.lock` hash;
- the selected installable and target system;
- the Nix version and relevant evaluation configuration;
- the derivation path and realized output paths;
- NAR hashes for the outputs and their recursive runtime closure, stored as a content-addressed closure manifest.

Recording realized NAR hashes matters because a store path identifies a build recipe but does not, by itself, establish that independently rebuilt output bytes are identical. Local flake inputs are bound by their evaluated source-tree hash rather than by `flake.lock` alone. Metewand's environment cache creates GC roots for resolved outputs; only explicit cache garbage collection removes them. Run records retain the complete fingerprint after removal, and Metewand may recreate the environment by rebuilding or substituting the locked installable, subject to source and cache availability.

Nix remains an environment provider, not an executor. Its build sandbox does not constrain the later benchmark process. CPU, memory, network, filesystem, and process-tree controls remain the responsibility of the selected executor and are reported with the same enforcement levels as every other environment backend. Dataset and run outputs remain Metewand artifacts rather than Nix store outputs.

Executor backends consume the launch specification:

```text
local process       required
OCI runtime         required on Linux via Docker or Podman
SLURM/remote        later
```

On Linux, executors should support CPU affinity, thread-related environment variables, process-tree memory and CPU accounting, a private writable run directory, read-only inputs, and disabled networking. Setting thread environment variables is not considered enforcement of a thread limit. The local process executor may report controls as unsupported; OCI or a later BenchExec backend may provide stronger enforcement. Do not build a generic sandbox or scheduler in v1; preserve an executor interface so stronger isolation or an HPC backend can be integrated later.

Performance runs are serial by default. The execution policy may randomize or interleave solver configurations deterministically to reduce ordering, thermal, and temporal bias. Concurrent execution is opt-in and becomes part of the run identity and provenance.

Artifacts live in a content-addressed cache. Downloaded files are verified before extraction. Local trees are hashed from sorted normalized paths, entry types, file bytes, symlink targets, and semantically relevant executable bits; timestamps and ownership are excluded. Archives are extracted with traversal, link-target, size, and entry-count checks.

A dataset derivation identity covers the dataset definition, verified sources, materializer source and environment, canonical parameters, and dataset seed. The materialized output receives a separate content hash. The cache records the mapping between them; in locked mode, regeneration must reproduce the expected output hash or fail.

Artifacts and run outputs are written to staging directories. On successful finalization, `metewand` validates their manifests, computes content hashes, atomically publishes them, and makes them read-only where supported. Later reads verify hashes at trust boundaries. “Immutable” therefore means that mutation is prevented when practical and always detected before a locked artifact is accepted.

Run outputs are immutable directories containing:

```text
run.json             experiment identity and full provenance
metrics.json         evaluator output
solver/              canonical solver result files
logs/                role- and attempt-scoped worker output
manifest.json        hashes and sizes of finalized output files
```

A top-level index is stored as JSON Lines or Parquet and can be exported to CSV. It includes successful, invalid, censored, and failed attempts; metrics are nullable and carry schema version, units, and optimization direction where applicable. The schema preserves pairing keys, component seeds, warm-up status, timeout limits, run order, machine/environment blocks, and the execution policy's authoritative timing measure. Metewand never silently averages repetitions or drops failed attempts.

Metewand owns this analysis-ready schema and its provenance, but publication analysis and graphics are not part of the execution core. The R and Python packages provide typed result readers and uncontroversial operations such as filtering warm-ups, pairing compatible attempts, and validating result-set compatibility. Statistical estimators and publication plots remain downstream code owned by the benchmark author.

A later analysis layer may produce immutable, provenance-tracked artifacts:

```text
AnalysisArtifact = result set + analysis definition + parameters + environment
```

Analysis artifacts do not alter the identity or validity of their source runs. This allows statistical choices to evolve without rerunning solvers while keeping the analysis reproducible.

Metewand may provide a read-only interactive diagnostic viewer over the same public result schema. Its purpose is to inspect run coverage, failures, parameter slices, raw metric distributions, timing, and provenance. Views are deliberately fixed and generic; the viewer does not provide a plotting grammar, themes, figure composition, annotations, statistical-test selection, or publication-format guarantees. It may export the selected underlying data, but publication figures are produced in R, Python, Julia, or another downstream tool.

## 7. Initial SDK APIs

The R and Python packages should be intentionally small and usable without knowledge of the orchestrator internals.

R sketch:

```r
metewand::serve_solver(
  prepare = function(instance, problem_parameters, solver_parameters, seed) { ... },
  solve = function(state, budget) { ... },
  result = function(state, directory) { ... }
)
```

Python sketch:

```python
from metewand import Solver, serve_solver

class SklearnSolver(Solver):
    def prepare(
        self, instance, problem_parameters, solver_parameters, seed
    ): ...
    def solve(self, budget): ...
    def write_result(self, directory): ...

serve_solver(SklearnSolver())
```

The SDKs also provide analogous, small helpers for dataset materializers and evaluators. They must not provision environments, acquire source artifacts, schedule runs, define parameter semantics, or calculate trusted metrics independently of user-supplied evaluator code.

## 8. MVP and acceptance criteria

The architectural MVP is a narrow vertical slice:

1. Rust CLI with manifest and schema validation, deterministic parameter expansion, stable identities, and typed run outcomes.
2. Side-effect-free planning, versioned JSON/JSONL command output, stable diagnostics and exit codes, append-only journals, and resumable runs.
3. Version-1 dataset, solver, and evaluator protocol over JSON Lines.
4. Existing local environments and the local process executor, with explicit reporting of unsupported controls.
5. Small R and Python SDKs plus raw command workers.
6. SHA-256-pinned downloads, hashed local/generated datasets, a content-addressed cache, and finalized run manifests.
7. Independent evaluation of canonical file-based results.
8. One-shot final-result benchmarking under a versioned execution policy.
9. Tidy JSONL/CSV export containing both successful and failed attempts; no dashboard.

The MVP proves the riskiest proposition: one benchmark definition can materialize parameterized data and compare parameterized R, Python, and native solvers fairly through a small process protocol.

Reproducible environment provisioning follows in two milestones:

1. **Locked environments:** `metewand.lock` and the Nix backend first, followed by `uv` and `renv`; Julia `Pkg` may follow later. Nix is the reference closure-locked backend but remains optional for benchmark users.
2. **Isolated archival execution:** OCI by digest, enforceable resource and network controls, and comprehensive machine provenance.

Problem-defined evaluation, iteration, and time budgets may be added after one-shot execution is reliable. Streaming checkpoints, trajectories, distributed execution, and richer scheduling remain later extensions.

The architecture is accepted when these examples require no special cases:

- **R-only:** compare `glmnet` and `ncvreg` under `renv`;
- **Python-only:** compare scikit-learn and `skglm` under `uv`;
- **Nix-locked:** resolve a flake installable once, launch its worker directly from the realized output, and reject source, lock, derivation, or closure mismatches;
- **Mixed:** compare an R package under `renv`, a Python package under `uv`, and a native executable from Nix;
- **Parameterized matrix:** dataset, problem, and solver grids expand deterministically, preserve their namespaces, and produce stable logical IDs and, after locking, stable resolved execution IDs;
- **Semantic equivalence:** every solver receives the same problem instance, and the independent evaluator rejects results that violate its problem contract;
- **Locked failure:** modifying a parameter, dataset, schema, adapter, native lockfile, flake source or lock, realized Nix closure, or image reference causes `run --locked` to refuse execution;
- **Control failure:** required isolation or resource controls fail during planning when the executor cannot enforce them;
- **Failure records:** crashes, timeouts, invalid results, and evaluator errors remain visible in exports;
- **Agent loop:** a noninteractive client can inspect a JSON plan, run selected stable IDs, diagnose structured failures, modify an adapter, and resume without repeating completed work;
- **Provenance:** every result identifies all software/data inputs and the machine on which it ran.

### Explicit non-goals for v1

- inventing a universal package manager or lockfile;
- embedding language runtimes or transferring native language objects;
- replacing general workflow engines, BenchExec, or SLURM;
- distributed execution, hosted benchmark registries, or a remote multi-user service;
- embedded language models or provider-specific agent orchestration;
- publication-ready plotting, figure composition, or reporting;
- bitwise-identical performance across hardware;
- automatic adapters for arbitrary optimization APIs.

The central invariant is:

> A benchmark run is valid only when its parameterized dataset, parameterized problem, parameterized solver, software environment, execution policy, observed controls, and resulting provenance can be identified independently of the language in which any worker is implemented.
