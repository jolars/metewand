# Metewand: Reproducible, Language-Neutral Computational Benchmarks

## 1. Purpose

`metewand` benchmarks computational methods as implemented in **software**, not merely their abstract algorithms. A benchmark may compare R packages, Python packages, Julia packages, native executables, or any mixture of them without privileging one host language.

The execution model covers bounded computations that map a configured dataset and problem to a canonical result that can be evaluated independently. Optimization, estimation, sampling, simulation, prediction, numerical integration, and similar scientific computations fit this model. Long-running services, interactive systems, request-throughput benchmarks, and distributed systems with no bounded result artifact do not.

The central object is an immutable experiment composed of:

```text
configured dataset + configured problem + configured implementation
    + software environment + execution policy
```

Datasets, problems, and implementations are parameterized definitions rather than fixed objects. A dataset is a reproducible family of materialized data artifacts; it may load recorded data, generate synthetic data, construct simulation state, select a fold, or apply benchmark-defined preprocessing. Expanding an experiment produces immutable dataset instances, problem instances, implementation configurations, and, ultimately, run specifications.

The tool must make simple, single-language work pleasant while supporting publication-grade runs with pinned datasets, pinned environments, controlled execution, and complete provenance.

### Design principles

1. **One language-neutral orchestrator.** The main product is a standalone binary; it never embeds R, Python, or Julia.
2. **Processes, not FFI.** Workers communicate through a versioned protocol over pipes. No `rpy2`, PythonCall, RCall, or embedded interpreters.
3. **Native workflows remain native.** An R-only benchmark may use `renv`; a Python-only benchmark may use `uv`; a Nix-based benchmark may provide any or all worker environments from flakes. None requires another environment backend.
4. **Reproducibility is explicit.** Datasets, source files, lockfiles, images, and configurations are hashed or pinned.
5. **Parameter namespaces are explicit.** Dataset, problem, implementation, budget, and execution parameters are never conflated.
6. **Evaluation is independent of implementations.** Implementations return canonical results; the problem's benchmark-owned evaluator computes correctness and quality metrics in a separate worker.
7. **Strictness is graduated.** Local exploratory, locked, and isolated execution modes make distinct, limited guarantees.
8. **Automation is a first-class interface.** The CLI is deterministic, noninteractive, inspectable, and machine-readable so the same workflows compose cleanly in shells, CI, and agent loops.

## 2. User model

A repository contains a manifest, optional native lockfiles, dataset materializers, problem contracts and evaluators, implementation adapters, schemas, and a generated lockfile:

```text
benchmark/
├── metewand.toml
├── metewand.lock
├── flake.nix                 # optional Nix environments
├── flake.lock
├── datasets/
│   └── leukemia.py
├── problems/
│   ├── lasso.toml
│   ├── lasso.py               # independent result evaluator
│   └── fixtures/
│       └── lasso-small/
│           ├── dataset/
│           ├── result/
│           └── metrics.json
├── implementations/
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
│   ├── lasso-semantics.json
│   ├── lasso-metrics.json
│   ├── lasso-result.json
│   ├── leukemia-dataset-parameters.json
│   ├── regression-dataset.json
│   ├── glmnet-parameters.json
│   └── sklearn-parameters.json
└── assets/                  # optional repository-local, hashed sources
```

The `datasets/` directory contains definitions that acquire, generate, or transform data. The optional `assets/` directory contains repository-local source files referenced by those definitions. Materialized dataset instances live in Metewand's content-addressed cache, not in either directory. A problem's evaluator is colocated with its contract because it is part of that problem, although Metewand launches it as an independent worker in its declared environment.

A representative manifest is:

```toml
version = 1
name = "lasso-comparison"

[problems.lasso]
contract = "problems/lasso.toml"

[problems.lasso.evaluator]
runner = "python"
entrypoint = "problems/lasso.py"
sources = ["problems/lasso.py"]
environment = "python"

[datasets.leukemia]
parameter_schema = "schemas/leukemia-dataset-parameters.json"
output_schema = "schemas/regression-dataset.json"
runner = "python"
entrypoint = "datasets/leukemia.py"
sources = ["datasets/leukemia.py"]
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
installable = ".#native-lasso"
system = "x86_64-linux"
output_kind = "package"
output = "out"

[implementations.glmnet]
runner = "r"
entrypoint = "implementations/glmnet.R"
sources = ["implementations/glmnet.R"]
environment = "r"
problem_contracts = ["lasso"]
parameter_schema = "schemas/glmnet-parameters.json"
capabilities = ["one_shot"]

[implementations.sklearn]
runner = "python"
entrypoint = "implementations/sklearn.py"
sources = ["implementations/sklearn.py"]
environment = "python"
problem_contracts = ["lasso"]
parameter_schema = "schemas/sklearn-parameters.json"
capabilities = ["one_shot"]

[implementations.native]
runner = "command"
program = "bin/native-lasso"
args = ["--metewand-worker"]
environment = "native"
problem_contracts = ["lasso"]
capabilities = ["one_shot"]

[[experiments]]
name = "main"
problem = "lasso"
datasets = ["leukemia"]
implementations = ["glmnet", "sklearn", "native"]
execution_policy = "controlled"
implementation_repetitions = 5
measurement_repetitions = 2
seed = 2025

[experiments.dataset_parameters.leukemia]
fold = { grid = [1, 2, 3] }
standardize = { value = true }

[experiments.problem_parameters]
lambda = { grid = [0.001, 0.01, 0.1] }
fit_intercept = { value = true }

[experiments.implementation_parameters.glmnet]
tolerance = { value = 1e-7 }

[experiments.implementation_parameters.sklearn]
selection = { grid = ["cyclic", "random"] }

[execution_policies.controlled]
version = 1
cpus = 1
threads = 1
memory = "8 GiB"
network = false
worker_reuse = false
warmup_runs = 0
timeout = "10 min"
timing_scope = "prepare_and_execute"
primary_time = "timed_wall_time"
run_order = "randomized"
enforcement = "best_effort"
```

The referenced problem contract is also concrete and reviewable. For example:

```toml
version = 1
name = "lasso"
family = "optimization"
parameter_schema = "schemas/lasso-problem-parameters.json"
dataset_schemas = ["schemas/regression-dataset.json"]
result_schema = "schemas/lasso-result.json"
metric_schema = "schemas/lasso-metrics.json"
semantics_schema = "schemas/lasso-semantics.json"
allowed_timing_scopes = ["cold_end_to_end", "prepare_and_execute"]
supported_budgets = ["none"]

[semantics.objective]
family = "least_squares_l1"
loss_scale = "1/(2*n)"
penalty = "lambda*sum(abs(coefficients))"
intercept_penalized = false

[semantics.data_conventions]
standardization = "encoded_in_dataset_instance"
intercept_column = "absent"

[semantics.preprocessing]
allowed_outside_timed_scope = []

[semantics.acceptance]
nonfinite = "reject"
objective_relative_tolerance = 1e-8
maximum_kkt_violation = 1e-6

[[reference_cases]]
dataset = "problems/fixtures/lasso-small/dataset"
result = "problems/fixtures/lasso-small/result"
expected_metrics = "problems/fixtures/lasso-small/metrics.json"
```

Primary commands:

```text
metewand init [path]            create a minimal benchmark manifest
metewand scaffold <component>   create boilerplate for one requested component
metewand check                 validate manifest, contracts, and schemas
metewand check --workers       launch workers and verify capabilities
metewand schema                print versioned public schemas
metewand plan                  show the expanded, side-effect-free run plan
metewand lock                  resolve artifacts and write metewand.lock
metewand run                   exploratory run; record what was used
metewand run --locked          refuse any lock or artifact mismatch
metewand status                inspect attempts and resumable state
metewand explain <id>          explain a specification, artifact, or failure
metewand export --format csv   export tidy results
```

### Authoring and scaffolding

`metewand init` creates a minimal, domain-neutral benchmark containing a
versioned manifest and no scientific definitions. It does not create datasets,
problem semantics, evaluators, fixtures, implementations, or software
environments.

Component scaffolding is explicit and incremental. `metewand scaffold dataset`,
`metewand scaffold problem`, and `metewand scaffold implementation` create only
the structural files required for the requested component and update the
manifest accordingly. Generated code may implement protocol and schema
boilerplate, but it must not choose problem semantics, acceptance criteria,
parameter meanings, reference results, or library-specific translations. Those
choices remain visible work for the benchmark author.

Metewand does not replace native environment initialization commands such as
`uv init` or `renv::init()`. Complete runnable benchmarks, including examples
such as a Python lasso comparison, are distributed under `examples/` rather
than exposed as privileged built-in templates.

Each parameter entry is either a literal `value` or an expansion `grid`. This distinction permits arrays and tables to be literal parameter values. Grids form a Cartesian product across the dataset, problem, and applicable implementation namespaces. In version 1, selected non-Cartesian combinations are expressed as separate named experiment entries; zipped axes may be added later.

### Schemas, canonical values, and identities

Version 1 uses JSON Schema 2020-12. Every schema declares its dialect, and every external `$ref` must resolve to another repository file named in the lockfile; schema validation never retrieves a remote reference. JSON Schema's `default` keyword remains an annotation and is never inserted by Metewand. A definition may instead declare a literal `parameter_defaults` object in the manifest; defaults cannot contain grids. Defaults are merged recursively by object key, while a supplied scalar, array, or `null` replaces the default at that key. Metewand performs this merge before validation, records the resolved object, and hashes only the resolved object.

Parameter values use a language-neutral JSON subset. Integers lie in `[-(2^53 - 1), 2^53 - 1]`; noninteger numbers are finite IEEE-754 binary64 values; negative zero is normalized to zero; and duplicate object keys are rejected. Strings are Unicode scalar values. Version 1 canonicalization follows RFC 8785 for this restricted domain. Schema validation occurs before canonicalization, and the exact canonical bytes are used for hashing and transport. The repository contains golden canonicalization vectors that every SDK must pass.

Every stable identity is a typed, versioned content identity:

```text
mw1-<kind>-<hex SHA-256(kind || NUL || canonical representation)>
```

The canonical representation refers to dependencies by their typed identities, forming a Merkle graph. It never contains workspace, staging, or cache paths, timestamps, or map iteration order. Backend-native immutable addresses, such as a locked Nix store program, may appear alongside their content hashes. Definition identities include their schemas, contracts, declared source bundles, and unresolved environment definitions. Resolved identities additionally include materialized artifact hashes, the resolved environments and launch programs for every worker role, the executor definition, the wire-protocol version, and a Metewand execution-semantics version. The exact Metewand build remains attempt provenance; a behavior change that can affect execution increments the semantics version.

The whole-manifest hash is computed from the versioned typed manifest after built-in defaults, path normalization, and name resolution, not from TOML spelling or comments. In `metewand.lock` it proves that the lock belongs to the current manifest, but it is not a dependency of each component identity. Consequently, adding an unrelated experiment invalidates the lockfile until it is regenerated without changing the identities of unaffected configurations or runs. Identity rules and golden vectors are part of the public compatibility contract.

Every interpreted worker declares `sources`, a list of repository-relative files or glob patterns that form its source bundle. Patterns are expanded in sorted bytewise path order, must match at least one entry, may not escape the repository through a path or symlink, and are hashed with the tree rules used for artifacts. Imported helpers, configuration files, and other runtime-read repository files must be included. Locked isolated executors expose only the declared bundle; weaker executors verify it before and after execution and record that undeclared-file access could not be prevented. Native code contained entirely in a resolved environment closure does not require a separate source bundle.

Parameter ownership is semantic rather than inherited from a library API:

- dataset parameters determine acquired, generated, selected, or transformed dataset artifacts, such as recorded data, simulation state, sample size, noise, preprocessing, or a fold;
- problem parameters determine the computation and result semantics, such as an estimand, loss, regularization strength, prediction target, integration domain, or simulation horizon;
- implementation parameters determine how one implementation executes that problem, such as tolerance, method variant, approximation, or internal initialization;
- budgets and execution parameters determine the comparison conditions without changing the problem or implementation configuration.

Every selected implementation configuration must execute the same problem instance. A library option that changes the requested computation, dataset transformation, target, or result interpretation is therefore a dataset or problem parameter even if the library presents it as an implementation option. Metewand validates declared schemas and namespaces, but semantic classification remains the benchmark author's responsibility and is reviewable in the problem contract.

The problem contract is a required, structured, hashed artifact rather than an integer label or evaluator convention. Its common envelope identifies the requested computation, accepted dataset schemas, parameter and canonical result schemas, metric schema, and supported timing scopes and budgets. The problem definition owns an independent evaluator worker, including its source bundle and environment; it is not a separately selectable benchmark component. The `family` value is an open, stable tag rather than a closed Metewand enumeration. A family-specific `semantics` object binds the remaining rules and is validated against the contract's `semantics_schema`. Metewand hashes this object but does not need to understand it. For example, an optimization contract binds its objective, constraints, scaling, preprocessing, and acceptance tolerances; other problem families bind their own domain-specific semantics. A contract may link explanatory prose, but every rule that affects execution or acceptance is represented in its validated structure. Each problem ships small reference instances and expected metrics that `metewand check --workers` evaluates as conformance tests.

Schema identifiers and contents are versioned and hashed. Before materialization or execution, the planner requires each dataset definition's output schema to match a schema accepted by the selected problem contract. Cross-schema conversion requires an explicit dataset materializer; it is never inferred from field names or shapes.

A problem may declare `dataset_schemas = []` when it requires no external dataset. Such an experiment must omit `datasets`; the planner binds one versioned, built-in unit dataset definition, configuration, and instance rather than producing zero runs or requiring a dummy user file. A problem that requires a dataset must select at least one dataset definition. Each selected definition expands into separate problem instances, each of which binds exactly one dataset instance. Problems that need several logical data components represent them in one structured dataset instance with a contract-owned schema.

Every implementation definition declares the problem contracts it implements. Manifest names are resolved to exact contract identities during planning, and that resolved set is part of the implementation-definition identity. The planner rejects any implementation/problem pairing outside the set; `prepare` receives the selected contract identity, and contract conformance fixtures test that declaration against the actual worker.

The experiment seed deterministically derives separately recorded dataset, implementation, and scheduling seeds using `SHA-256("metewand-seed-v1" || NUL || role || NUL || canonical seed fields)`. The dataset-seed fields are the experiment seed, dataset-definition identity, and resolved dataset parameters. The implementation-seed fields are the experiment seed, dataset- and problem-configuration identities, and implementation-replication index; they deliberately exclude the implementation identity. The scheduling-seed fields are the experiment seed, benchmark and experiment names, and scheduling-policy version, but not the expanded member set. The first 53 digest bits are exposed to workers as a nonnegative JSON integer, and the complete derivation digest is recorded. No derivation depends on expansion order. Implementations in the same problem-instance and implementation-replication block therefore receive the same numeric seed, although Metewand does not claim that different random-number generators produce paired random streams.

`implementation_repetitions` creates distinct logical run specifications by varying the implementation seed. `measurement_repetitions` creates distinct logical attempt slots for the same logical specification and seed; resolution later binds each slot to a resolved specification. Dataset replication is expressed through an explicit parameter such as `replicate`, which participates in the dataset seed. Version 1 has no hidden problem seed: random quantities that define the requested computation must appear explicitly in canonical problem parameters or be materialized into the dataset instance. A problem `replicate` value is therefore an ordinary explicit parameter interpreted by the contract. Deterministic scheduling assigns pseudorandom block and within-block priorities derived from the scheduling seed and typed block or slot identity, then sorts by those priorities. Adding an unrelated slot therefore does not reorder existing slots relative to one another.

## 3. Core architecture

Metewand is a Rust workspace with one distributable executable and small optional language SDKs:

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
- problem-owned evaluator invocation;
- provenance and result storage.

Language SDKs own only:

- protocol serialization;
- dispatch to user-supplied callbacks/classes;
- consistent error reporting;
- monotonic timing around the implementation call.

Any executable may implement the protocol directly; an SDK is never required.

### Automation and agentic workflows

The command-line interface is the universal automation API. It must work without a terminal, browser, editor integration, or model-specific plugin. Claude, Codex, other agents, CI jobs, and ordinary scripts receive the same semantics; an optional MCP adapter may later expose these operations without creating a second behavioral API.

Every command supports a versioned machine-readable output mode. In that mode, standard output contains only JSON or JSON Lines conforming to published schemas; progress and human diagnostics go to standard error. Exit codes and diagnostic codes are stable and documented. Structured diagnostics include source spans, affected stable identities, causal chains, and concrete remediation where it can be stated safely. Human-readable output is rendered from the same typed records rather than maintained as a separate source of truth.

Commands are noninteractive by default and never silently rewrite manifests, native lockfiles, or source files. An operation that requires an additional mutation or capability fails with an explanation and the explicit flag or command that authorizes it. Destructive cleanup is a separate command with a dry-run mode and exact artifact targets.

The authoring commands are the deliberate exception to the default read-only
posture. `init` and `scaffold` report every path they create or modify, support a
dry-run mode, and fail rather than overwrite an existing file. Scaffolding may
edit the manifest only as part of the component addition explicitly named by
the user; it never invokes an ecosystem package manager or modifies a native
lockfile.

The read-only `plan` command performs schema validation, deterministic expansion, declared-capability compatibility checks, and logical run-ID assignment without downloads, builds, worker launches, or filesystem mutation. Worker and executor capabilities in a plan are explicitly labeled `declared`, not verified. `check --workers`, `lock`, and `run` may launch workers and preflight executors; they require every declared capability used by the plan to be observed and label it `verified_now` or fail. Additional observed capabilities are recorded but not selected implicitly. A lock records the complete capabilities observed during resolution as `previously_verified`, and `run` verifies the selected subset again before use. This distinction prevents a side-effect-free plan from claiming knowledge that requires execution.

Resolution through `lock` adds artifact and environment fingerprints and produces a separate resolved execution ID. This distinction keeps plans inspectable before expensive provisioning without pretending that an unresolved environment is known. Its output includes:

- every dataset, problem, implementation, logical run, and logical attempt-slot identity;
- the number and order of planned slots;
- required artifacts, environment resolutions, executors, and controls;
- anticipated network access, builds, commands, and writable paths;
- unsupported capabilities and blockers known before execution, with each capability's verification state.

Long-running operations emit append-only structured events. An authoritative SQLite database in WAL mode with full synchronous durability coordinates resolved attempt slots, leases, transitions, and finalized artifact references; event rows are insert-only, and state changes occur in transactions. A uniqueness constraint permits at most one accepted attempt per resolved slot. Before writing its immutable outcome record, a successful candidate atomically acquires a leased finalization claim; a concurrent loser is finalized as superseded. The claim alone never satisfies the slot: acceptance becomes terminal only after durable artifact publication and the final database transaction. Recovery resolves an expired claim from its completed directory or marks the interrupted attempt failed before another candidate can claim the slot. The run store checks required locking, synchronization, and atomic-rename behavior at initialization and refuses durable mode on an unsupported filesystem. Immutable attempt directories remain self-describing, so the database and export indexes can be rebuilt from finalized artifacts. `run --resume` fills unsatisfied resolved slots without repeating accepted measurements. A failed slot is retried only with an explicit retry policy or flag, and every retry remains a distinct `RunAttempt`. Interrupted staging directories carry leases and are either safely resumed by their owner or quarantined after the lease expires. Filters operate on stable logical or resolved run, slot, or attempt identities, and explicit bounds such as maximum attempts or selected IDs let an agent test a small change before expanding to the complete benchmark.

This interface is agent-friendly, not agent-dependent. Metewand does not embed a model, send source or results to an external service, generate scientific conclusions, or grant an agent authority beyond the invoked command. All operations remain usable and auditable as ordinary local CLI calls.

### Main domain types

```text
Benchmark                   named collection of experiments
DatasetDefinition           source or generator, materializer, and parameter schema
DatasetConfiguration        definition + canonical parameters + dataset seed
DatasetInstance             immutable artifact materialized from a dataset configuration
ProblemDefinition           semantic contract, parameter and result schemas, and evaluator worker
ProblemConfiguration        definition + canonical parameters
ProblemInstance             dataset instance + problem configuration
ImplementationDefinition    adapter, supported contracts, parameter schema, and capabilities
ImplementationConfiguration definition + canonical parameters + selected environment
Environment                 reproducible software context for a worker
Executor                    mechanism that launches and constrains a worker
ExecutionPolicy             resources, isolation, timing policy, and repetition policy
LogicalRunSpecification
                      dataset + problem + implementation configurations + budget + policy
                      + implementation-replication index + implementation seed
ResolvedRunSpecification
                      logical specification + materialized dataset instance + resolved source,
                      environment, launch, problem evaluator, and executor artifacts
LogicalAttemptSlot    logical specification + measurement index + warm-up role + schedule priority
ResolvedAttemptSlot   logical slot + resolved specification
RunAttempt            resolved slot + retry identity + one observed execution
```

Definitions describe parameterized families, and configurations bind canonical parameter values. Logical specifications and slots can therefore be identified before acquisition or environment provisioning. Resolution binds immutable artifacts and environment fingerprints into corresponding resolved identities containing every declared dependency needed for execution. A `ResolvedAttemptSlot` represents one planned statistical observation under one resolved dependency set; it has at most one accepted attempt but may retain multiple failed or superseded retries. A `RunAttempt` adds observed controls, machine context, state, timestamps, and results without changing the logical or resolved identities.

`ImplementationDefinition` and `Environment` remain deliberately separate. The same adapter may run locally during development and in an OCI image for an archival run.

### Problem and fairness contracts

A problem definition references the required versioned contract and owns its executable evaluator rather than using a name as a semantic assertion. Together, they bind:

- accepted dataset and problem-parameter schemas;
- canonical result and metric schemas;
- the evaluator worker and its environment;
- the requested computation and domain-specific conventions;
- supported budget types and their semantics;
- correctness tolerances and rules for invalid results.

The contract remains language-neutral and reviewable, while its evaluator may be implemented in any supported language and runs through the worker protocol. This is a runtime boundary, not another experiment axis: an evaluator is owned by exactly one problem definition and cannot be selected independently.

Every experiment also selects a versioned execution policy. The policy specifies worker reuse, permitted caching, warm-up runs, run ordering, timeouts, resource requirements, and one timing scope:

- `cold_end_to_end`: time from immediately before worker launch through receipt of the complete canonical result;
- `prepare_and_execute`: launch and handshake first, then time `prepare`, `execute`, and canonical result writing;
- `execute_only`: time only `execute` and canonical result writing; permitted only when the problem contract precisely defines allowed untimed preparation.

The authoritative `timed_wall_time` is measured by the orchestrator around the complete selected scope. The policy's scientific `timeout` applies to that same scope. Materialization, untimed startup or preparation, evaluation, reset, and shutdown have separate operational timeouts with fully resolved defaults, so no worker phase can hang indefinitely. Executor-observed CPU time may be another primary measure only when it covers the same process tree and scope. In-worker `implementation_time` is always secondary diagnostic provenance; it cannot be the sole authoritative comparison measure. Metewand-owned validation and evaluation occur after the timed scope. Compilation, preprocessing, conversion, and result serialization are included whenever they occur inside the selected scope, and the run record identifies excluded phases explicitly.

Exports label `execute_only` observations as operation-scope benchmarks rather than end-to-end software measurements, and compatibility readers never pool different timing scopes implicitly.

`cold_end_to_end` requires `worker_reuse = false` and `warmup_runs = 0`, because every measured slot must launch a fresh process. Process-local warm-ups require worker reuse and execute on the same worker before its measured slots. Version 1 rejects `warmup_runs > 0` with `worker_reuse = false`; system-cache or thermal conditioning must instead be an explicitly named later policy feature. A discarded worker's warm-ups never satisfy a replacement worker. These rules make the timing claim mechanically observable, while the independent evaluator makes the result checkable.

Work-unit counts are not presumed comparable across implementations. Evaluation, iteration, sample, and time budgets are permitted only when the problem contract defines their meaning and the worker declares the corresponding capability. Otherwise, the benchmark compares final results under explicit implementation completion or stopping rules.

## 4. Worker protocol

Use a versioned JSON-Lines protocol over two dedicated unidirectional pipes. Standard output and standard error are drained concurrently into bounded worker logs, so unexpected library output cannot corrupt or deadlock the protocol. Version 1 targets POSIX systems and passes the worker's inherited read and write descriptor numbers through `METEWAND_PROTOCOL_READ_FD` and `METEWAND_PROTOCOL_WRITE_FD`; a later version may specify Windows handle transport. SDKs receive the protocol descriptors directly. Raw command workers may explicitly request a stdin/stdout compatibility mode, in which case any unexpected standard output is a protocol violation. Large arrays and dataset artifacts are passed as paths to private artifact views, never embedded in JSON.

Protocol JSON uses the same numeric and Unicode subset as canonical parameters. Messages are UTF-8 without a byte-order mark, contain no duplicate keys, end in one newline, and are limited to 1 MiB in version 1. Schemas reject unknown fields; adding a field therefore requires a negotiated protocol version. Oversized lines, invalid encoding, malformed JSON, early EOF, extra responses, and mismatched request IDs are typed protocol failures. Logs have policy-defined byte limits. Once a limit is reached, Metewand records truncation and continues draining to a sink so the child cannot block on a full pipe.

Every request and response carries a string request ID. Version negotiation, role, resolved worker identity, SDK version, and capabilities are established before any work begins. Version 1 permits at most one outstanding request per worker, but IDs make timeouts, late responses, and diagnostics unambiguous. The orchestrator supplies a minimal allowlisted environment, fixed working directory, locale, timezone, and umask; `HOME`, temporary directories, and ecosystem cache directories point to private worker or attempt storage so user configuration and caches are not inherited accidentally. Their resolved templates and behavior-affecting values are recorded, while ephemeral absolute paths are attempt provenance rather than identity inputs. Secrets are passed only through explicitly declared redacted channels and are never included verbatim in plans, logs, identities, or provenance.

The orchestrator begins a worker session with a handshake:

```json
{"id":"0","method":"hello","protocols":[1],"role":"implementation","worker_id":"mw1-worker-..."}
```

The worker responds with supported capabilities, such as:

```json
{"id":"0","ok":true,"protocol":1,"worker_id":"mw1-worker-...","sdk":{"name":"metewand-python","version":"0.1.0"},"capabilities":["one_shot"]}
```

Direct protocol workers return `sdk: null`. The orchestrator rejects a worker identity mismatch or a worker missing any selected declared capability; additional capabilities are inert unless a later plan selects them.

Dataset materializers support:

```text
materialize(source_dir, dataset_parameters, dataset_seed, output_dir)
shutdown()
```

The materializer writes a canonical dataset manifest and files beneath `output_dir`. The manifest records every file's relative path, media type, byte hash, size, and, where relevant, logical shape, data type, byte order, and sparse-layout convention. Metewand validates the manifest schema, verifies every declared file, rejects undeclared entries, and runs any problem-contract dataset validator before publication. JSON Schema alone is not treated as validation of opaque binary data. A dataset definition without a materializer treats its verified source tree as the dataset instance and therefore cannot accept materialization parameters.

Mandatory implementation operations:

```text
prepare(dataset_dir, problem_contract_id, problem_parameters, implementation_parameters, implementation_seed)
execute(result_dir, optional_budget)
reset()
shutdown()
```

`execute` returns scalar metadata and a manifest of files under `result_dir`:

```json
{
  "id": "2",
  "ok": true,
  "implementation_time_ns": 18342011,
  "result": {"manifest": "result.json"},
  "statistics": {"iterations": 37}
}
```

The canonical result schema belongs to the problem contract. For a lasso problem it might require `coefficients` and `intercept`, regardless of how individual libraries represent them. An `execute` response is sent only after canonical result files have been closed and made visible. Their manifest uses the same path, hash, size, media-type, and representation metadata as dataset manifests. Result serialization is therefore inside every timing scope that includes `execute`.

Problem evaluator workers support:

```text
evaluate(dataset_dir, problem_contract_id, problem_parameters, result_dir, metrics_path)
```

They compute problem-defined quality and correctness metrics—for example, objective values, sampling diagnostics, prediction error, numerical error, or constraint violations. The orchestrator validates the completed metrics document against the contract's metric schema and acceptance rules before it can accept the attempt. Implementation-reported metric values may be stored as diagnostics but are not authoritative.

Workers may be persistent according to the execution policy so interpreter startup and package loading can be separated from problem execution. `worker_reuse = false` provides a fresh process but is called process freshness, not strong sandbox isolation. The reuse key includes role, definition, source bundle, resolved environment, launch program, and policy. The orchestrator calls `reset` between slots, clears slot-scoped writable directories, and requires every `prepare` to establish clean state. Any cache allowed to persist across slots is named explicitly by the policy and included in the reuse semantics. A worker that times out, crashes, violates the protocol, or fails to reset is discarded. If process-local warm-ups were requested, its replacement repeats them before accepting a measurement.

A run attempt advances through `reserved`, `materializing`, `starting`, `preparing`, `executing`, `validating`, `evaluating`, and `finalizing` states. `accepted` and each typed failure are terminal attempt outcomes. A slot is satisfied only by an `accepted` attempt. State transitions and timestamps are transactional, but success becomes visible only after the finalized attempt directory has been durably published.

The orchestrator validates every referenced path and output manifest, rejects absolute paths, path traversal, special files, escaping links, and undeclared files, and never trusts a worker-supplied path outside its assigned directories. Each attempt receives a private dataset view and output directory. Strong executors expose immutable datasets with read-only mounts. The local executor prefers copy-on-write snapshots or copies; if it can provide only permission-based read-only views, Metewand verifies all dataset artifacts immediately before and after each worker operation and records immutability enforcement as best-effort. Any mutation invalidates the attempt and discards its private view; if the canonical cache object itself changed, Metewand quarantines it before another run can consume it.

Errors have stable machine-readable codes, a human-readable message, and optional structured details. The orchestrator distinguishes at least invalid configuration, unsupported or mismatched capability, materialization failure, setup failure, timeout, resource-limit violation, dataset mutation, worker crash, malformed protocol, invalid result, evaluator failure, persistence failure, and internal error. A run attempt records one of these outcomes rather than disappearing because it did not produce metrics. A timeout initiates an executor-defined termination sequence—interrupt, bounded grace period, then forced process-tree termination. An executor that cannot contain and terminate the complete process tree reports that limitation before execution and cannot satisfy a required process-control policy. Partial outputs are retained only as diagnostic artifacts and never accepted as canonical results.

The first release requires one-shot executions without a budget; the optional budget field is reserved and must be absent. Repeated fresh runs at problem-defined iteration, evaluation, sample, or time budgets, streaming checkpoints, and callback-based trajectories are later protocol extensions.

The protocol and executor are not a security boundary for hostile benchmark code. Benchmark repositories and workers are trusted inputs; isolation controls protect reproducibility and limit accidental interference. Running untrusted submissions requires a separately designed sandbox or an integration such as BenchExec.

## 5. Reproducibility model

`metewand.lock` is a **lockfile of definitions, configurations, artifacts, and native lockfiles**. It does not replace `renv.lock`, `uv.lock`, `flake.lock`, Julia `Manifest.toml`, or OCI manifests; it records and binds their identities into the benchmark.

It must contain at least:

- normalized manifest and execution-policy hashes;
- hashes of dataset materializers, each problem's contract and evaluator, schemas, implementation adapters, and every declared source bundle;
- canonical dataset, problem, and implementation configuration identities;
- dataset source hashes, materialization recipes, and output-tree hashes;
- hashes of native environment lockfiles;
- resolved runtime and package-manager requirements and fingerprints;
- for Nix environments, every evaluated local flake-input source identity, the flake lock, installable, output kind, target system, derivation, resolved program, realized outputs, and the digest of a recursive closure manifest;
- OCI image digests; tags alone are invalid in locked mode;
- resolved launch programs, allowlisted environment values, executor definitions, the exact Metewand build fingerprint, execution-semantics version, SDK versions, and wire-protocol versions;
- generator sources, parameters, seeds, environment identity, and expected output hash for generated datasets.

Artifact resolution may use the network. Actual benchmark runs default to no network access.

Four environment-resolution classes are supported:

| Mode | Environment | Guarantee |
|---|---|---|
| Development | Existing local runtime | Observed provenance only; convenient, but not reliably recreatable |
| Ecosystem-locked | `uv`, `renv`, later Julia `Pkg` | Declared dependencies and sources must match their locks; undeclared system and platform dependencies remain recorded limitations |
| Closure-locked | Nix flake installable | Evaluated source, derivation, realized store outputs, and dependency closure are identified; runtime kernel and hardware remain external |
| Image-pinned | OCI image by digest | Immutable declared userspace image; host kernel, hardware, runtime, and explicitly mounted datasets remain external |

Environment resolution and execution isolation are independent dimensions. A digest-pinned image does not by itself prove that networking, memory, CPU use, or filesystem writes were constrained. Each executor advertises its controls, and each run records every control as requested, enforced, best-effort, or unsupported. With `enforcement = "required"`, planning fails if a declared executor capability is known to be insufficient; runtime preflight or enforcement failure still produces a typed failed attempt because host permissions and facilities may change after planning. Best-effort execution remains available but is never presented as equivalent to enforced execution.

Unless a contract states a narrower rule, resource, network, filesystem, and process-tree controls begin before worker launch and cover its entire lifetime, including untimed startup, evaluation workers, reset, and shutdown. Excluding a phase from performance timing never excludes it from isolation or accounting provenance.

An attempt is `accepted` only if all of the following hold:

1. every declared dependency resolves to its expected typed identity, and every resolved identity matches the selected lock state when locked mode was requested;
2. the worker completed the negotiated protocol and produced a schema-valid canonical result manifest;
3. the problem's evaluator accepted the result under its contract;
4. every control marked required was observed as enforced throughout its applicable scope;
5. the finalized artifact and provenance record were durably published.

Acceptance is distinct from the environment reproducibility class and the observed control class. An accepted development or best-effort attempt may be useful, but exports label its limitations and compatibility readers do not silently pool it with closure-locked, fully enforced attempts. Required provenance fields may be selected by policy; an unavailable required field fails preflight, while an unavailable optional field is represented explicitly.

The tool must not claim bitwise-identical results or identical performance across machines. Every run records hardware and operating context, including CPU model, architecture, selected cores, memory, kernel, container runtime, language runtime, BLAS implementation, relevant thread variables, tool version, repository commit, and dirty-tree hash. Where available, it also records CPU governor, frequency/turbo state, NUMA placement, accelerator identity, and background-load indicators. Missing provenance is represented explicitly rather than silently omitted.

Timing fields are distinct:

```text
materialization_time   dataset generation or transformation
worker_startup_time    interpreter and package loading
prepare_time           the complete prepare request
implementation_time    monotonic in-worker timing around the implementation call
execute_request_time   executor-observed execute request and result serialization
timed_wall_time        orchestrator-observed selected timing scope
evaluation_time        independent result evaluation
cpu_time / peak_rss    executor-observed process-tree use where supported
```

The execution policy identifies a primary executor-observed timing measure covering its complete timing scope. Measurements that cannot include the complete worker process tree are marked partial and cannot satisfy a policy that requires complete accounting. Warm-up and untimed preparation are recorded even when excluded from the primary measure.

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

Every backend returns an exact executable or interpreter, argument prefix, allowlisted environment, and immutable environment fingerprint. For `uv` and `renv`, the native lockfile is necessary but not sufficient: resolution also records the interpreter, package-manager version, platform tags, repository identities, hashes of every downloaded wheel, archive, or source checkout, and a manifest of the realized environment. Local or editable dependencies are declared source bundles. Locked execution uses the already realized environment without network access and verifies its manifest before launch.

### Nix environments

Nix is a first-class locked environment backend. A Nix environment names a flake reference, an installable, a target system, and an `output_kind` of `package` or `app`. The output kind selects the exact `packages.<system>` or `apps.<system>` attribute namespace; Metewand does not reproduce the Nix CLI's fallback search between namespaces. It may provide an environment for a dataset materializer, problem evaluator, implementation, or later analysis worker. Standard flake outputs are the integration boundary, and Metewand does not require a separate `devenv` backend.

For a package output, resolution realizes the selected derivation outputs, and a command worker's `program` is a normalized path relative to one designated output root. For an app output, resolution evaluates the app's store-resident `program`; the worker definition owns only its arguments and protocol role. In both cases, Metewand validates that the resolved program is an executable regular file in the realized closure and locks its exact store path. It never guesses a binary name from a derivation name.

Resolution realizes the closure before any benchmark timing and returns the exact launch program and environment. Workers are launched directly from that program; `nix run`, `nix develop`, evaluation, substitution, and build time are never included in worker startup or problem-execution timing.

In locked mode, Metewand forbids impure evaluation and refuses any operation that would create, update, or rewrite `flake.lock`. The environment fingerprint records:

- the exact evaluated source hash of the root and every transitive local or `path:` flake input, plus the `flake.lock` hash;
- the selected installable, output kind, target system, designated output, and resolved program;
- the Nix version and relevant evaluation configuration;
- the derivation path and realized output paths;
- NAR hashes for the outputs and their recursive runtime closure, stored as a content-addressed closure manifest.

Recording realized NAR hashes matters because a store path identifies a build recipe but does not, by itself, establish that independently rebuilt output bytes are identical. Every local flake input is bound by the exact filtered source imported by Nix rather than by `flake.lock` alone. Metewand supports an explicit tested range of Nix versions and rejects versions outside it in locked mode because flake and installable CLI behavior is not a stable integration API. Metewand's environment cache creates GC roots for resolved outputs; only explicit cache garbage collection removes them. Run records retain the complete fingerprint after removal, and Metewand may recreate the environment by rebuilding or substituting the locked installable, subject to source and cache availability.

Nix remains an environment provider, not an executor. Its build sandbox does not constrain the later benchmark process. CPU, memory, network, filesystem, and process-tree controls remain the responsibility of the selected executor and are reported with the same enforcement levels as every other environment backend. Dataset artifacts and run outputs remain Metewand artifacts rather than Nix store outputs.

Executor backends consume the launch specification:

```text
local process       Gate 2
OCI runtime         Gate 4 on Linux via Docker or Podman
SLURM/remote        later
```

On Linux, executors should support CPU affinity, thread-related environment variables, process-tree memory and CPU accounting, a private writable run directory, read-only datasets, and disabled networking. Setting thread environment variables is not considered enforcement of a thread limit. The local process executor may report controls as unsupported; OCI or a later BenchExec backend may provide stronger enforcement. Do not build a generic sandbox or scheduler in v1; preserve an executor interface so stronger isolation or an HPC backend can be integrated later.

Performance runs are serial by default. `run_order = "randomized"` forms comparison blocks by dataset instance, problem configuration, implementation-replication index, and measurement index; it orders implementation configurations within each block and the blocks themselves by their deterministic scheduling priorities. This interleaves comparable implementations without making seeds or identities depend on completion order. A sequential order remains available when explicitly requested. Concurrent execution is opt-in and becomes part of the run identity and provenance because contention changes the comparison conditions.

Artifacts live in a content-addressed cache. Downloaded files are verified before extraction. Local trees are hashed from sorted normalized paths, entry types, file bytes, symlink targets, and semantically relevant executable bits; timestamps and ownership are excluded. Archives are extracted with traversal, link-target, size, and entry-count checks; device nodes, sockets, FIFOs, escaping hard links, and unsupported entry types are rejected.

A dataset derivation identity covers the dataset definition, verified sources, materializer source and environment, canonical parameters, and dataset seed. The materialized output receives a separate content hash. The cache records the mapping between them; in locked mode, regeneration must reproduce the expected output hash or fail.

Artifacts and run outputs are written to uniquely leased staging directories on the same filesystem as their destination. Finalization validates the complete manifest, computes content hashes, synchronizes every file and directory, publishes with an atomic no-replace rename, and synchronizes the parent directory before committing the terminal database transaction. If another writer won the same content address, Metewand verifies the existing object byte for byte and discards its duplicate staging object. Normal operations expose a finalized object only when its manifest contains a completion marker written last and the database references the same content identity. Recovery may validate and import a completed orphan left by a crash between publication and the terminal transaction.

Later reads verify hashes whenever data crosses from the cache into a worker view, before evaluation, and during export or explicit cache verification. “Immutable” therefore means that mutation is prevented when practical and detected before an artifact can contribute to an accepted attempt. A corrupted object is quarantined transactionally. Cache garbage collection takes a database snapshot, respects active leases and GC roots, and deletes only exact unreferenced content identities; an index or failed cleanup can never authorize deletion by a broad path.

Finalized attempt outputs are immutable directories containing:

```text
attempt.json         slot, retry, outcome, identities, controls, and full provenance
metrics.json         evaluator output when available
result/               canonical implementation result files when available
logs/                 role- and attempt-scoped worker output
manifest.json        completion marker plus hashes and sizes of all other files
```

A top-level JSON Lines or Parquet index is a rebuildable projection of the metadata database and finalized attempt directories, never authoritative state. It includes accepted, invalid, censored, superseded, and failed attempts; metrics are nullable and carry schema version, units, and comparison direction where applicable. The schema preserves slot and retry identities, pairing keys, component seeds, warm-up status, timeout limits, run order, machine/environment blocks, observed controls, guarantee classes, and the execution policy's authoritative timing measure. Metewand never silently averages repetitions or drops failed attempts.

Metewand owns this analysis-ready schema and its provenance, but publication analysis and graphics are not part of the execution core. The R and Python packages provide typed result readers and uncontroversial operations such as filtering warm-ups, pairing compatible attempts, and validating result-set compatibility. Statistical estimators and publication plots remain downstream code owned by the benchmark author.

A later analysis layer may produce immutable, provenance-tracked artifacts:

```text
AnalysisArtifact = result set + analysis definition + parameters + environment
```

Analysis artifacts do not alter the identity or acceptance status of their source attempts. This allows statistical choices to evolve without rerunning implementations while keeping the analysis reproducible.

Metewand may provide a read-only interactive diagnostic viewer over the same public result schema. Its purpose is to inspect run coverage, failures, parameter slices, raw metric distributions, timing, and provenance. Views are deliberately fixed and generic; the viewer does not provide a plotting grammar, themes, figure composition, annotations, statistical-test selection, or publication-format guarantees. It may export the selected underlying data, but publication figures are produced in R, Python, Julia, or another downstream tool.

## 7. Initial SDK APIs

The R and Python packages should be intentionally small and usable without knowledge of the orchestrator internals.

R sketch:

```r
metewand::serve_implementation(
  prepare = function(
    dataset_instance, problem_contract, problem_parameters, implementation_parameters, seed
  ) { ... },
  execute = function(state, budget) { ... },
  result = function(state, directory) { ... }
)
```

Python sketch:

```python
from metewand import Implementation, serve_implementation

class SklearnImplementation(Implementation):
    def prepare(
        self, dataset_instance, problem_contract, problem_parameters,
        implementation_parameters, seed
    ): ...
    def execute(self, budget): ...
    def write_result(self, directory): ...

serve_implementation(SklearnImplementation())
```

The SDK may report monotonic `implementation_time` around only the user execute callback, but it writes the canonical result before replying to the `execute` request; the orchestrator's timing envelope therefore includes serialization. The SDKs also provide analogous, small helpers for dataset materializers and problem evaluators. They must not provision environments, acquire source artifacts, schedule runs, define parameter semantics, or calculate trusted metrics independently of user-supplied evaluator code.

## 8. MVP and acceptance criteria

Implementation proceeds through explicit acceptance gates.

### Gate 1: conformance kernel

1. Parse and validate manifests, problem contracts, schemas, canonical parameter values, source bundles, identities, and seed derivations.
2. Expand a deterministic run matrix into logical specifications and attempt slots.
3. Implement the bounded version-1 protocol and raw fixture workers for dataset materializer, implementation, and problem evaluator roles.
4. Run a fixed local dataset through one raw implementation and its problem's independent evaluator under `prepare_and_execute` timing.
5. Finalize one self-describing attempt directory and emit versioned machine-readable command output.

This gate proves the riskiest architectural proposition without environment provisioning, downloads, SDKs, resume, or a general cache.

### Gate 2: reliable local MVP

1. Add pinned downloads, safe extraction, generated datasets, the content-addressed cache, private dataset views, and mutation checks.
2. Add the transactional attempt-slot database, durable finalization, crash recovery, explicit retries, cache leases, and rebuildable indexes.
3. Add the local process executor, capability preflight, complete timing scopes, process-tree termination where supported, and explicit unsupported-control reporting.
4. Add small R and Python SDKs, raw command workers, and reference problem-contract conformance suites.
5. Add deterministic block-randomized scheduling, implementation and measurement replication, selected-ID execution, status, explanation, and JSONL/CSV export containing all outcomes.

This is the first release called an MVP. It supports reproducible dataset artifacts and execution records but makes only development-environment and observed-control claims.

### Gate 3: locked environments

Add `metewand.lock` and the Nix package/app backend first, followed by `uv` and `renv`; Julia `Pkg` may follow later. Nix is the reference closure-locked backend but remains optional for benchmark users. Locked mode is accepted only after transitive source-bundle and environment-closure mismatch tests pass.

### Gate 4: isolated archival execution

Add OCI images by digest, enforceable resource and network controls, read-only mounts, complete process-tree accounting, and comprehensive machine provenance. This gate, not the local MVP, supports the strongest publication-archive claim.

Problem-defined evaluation, iteration, sample, and time budgets may be added after one-shot execution is reliable. Streaming checkpoints, trajectories, distributed execution, and richer scheduling remain later extensions.

The user-facing architecture is accepted when these examples require no special cases:

- **R-only:** compare `glmnet` and `ncvreg` under `renv`;
- **Python-only:** compare scikit-learn and `skglm` under `uv`;
- **Nix-locked:** resolve a flake installable once, launch its worker directly from the realized output, and reject source, lock, derivation, or closure mismatches;
- **Mixed:** compare an R package under `renv`, a Python package under `uv`, and a native executable from Nix;
- **Posterior sampling:** compare implementations that return canonical draws under a problem contract defining the target distribution, chain semantics, sample budget, and evaluator-owned diagnostics;
- **Dataset-free computation:** compare implementations that integrate a contract-defined function over problem-parameterized domains; omitting `datasets` produces one run per remaining configuration through the built-in unit dataset;
- **Parameterized matrix:** dataset, problem, and implementation grids expand deterministically, preserve their namespaces, and produce stable logical and resolved run and slot IDs plus immutable attempt IDs;
- **Semantic equivalence:** every implementation receives the same problem instance, and its independent evaluator rejects results that violate the problem contract;
- **Locked failure:** modifying a parameter, dataset, schema, contract fixture, adapter or imported helper, native lockfile, local flake input or lock, realized Nix closure, resolved program, or image reference causes `run --locked` to refuse execution;
- **Control failure:** a statically unsupported required control fails planning, while lost runtime capability or enforcement produces a typed failed attempt;
- **Failure records:** crashes, timeouts, invalid results, and evaluator errors remain visible in exports;
- **Agent loop:** a noninteractive client can inspect a JSON plan, run selected stable IDs, diagnose structured failures, modify an adapter, and resume without duplicating an accepted slot;
- **Provenance:** every result identifies all declared software and dataset artifacts, the machine on which it ran, and any control that could not prevent undeclared access.

The following automated tests are release-blocking:

- canonicalization, identity, and seed golden vectors agree in Rust, R, and Python;
- reordering tables or files does not change identities, while changing any transitive dependency does;
- adding an unrelated experiment leaves existing component and run identities unchanged;
- an `A x M` implementation-by-measurement design creates exactly `A x M` slots, shares seeds only where specified, and preserves every retry;
- timing fixtures that sleep or work in startup, `prepare`, `execute`, and result writing are included or excluded exactly according to each timing scope;
- reference problem instances produce expected metrics, and deliberately malformed, numerically wrong, or semantically invalid results are rejected;
- a dataset-free problem expands through exactly one built-in unit dataset and rejects selected dataset definitions, while a dataset-requiring problem rejects zero selections and expands multiple selections into separate single-dataset problem instances;
- protocol tests cover fragmented input, duplicate keys, invalid UTF-8, oversized messages, early EOF, extra responses, log floods, timeouts, and escaped child processes;
- fault injection terminates the process at every database, file-sync, rename, and finalization boundary, after which resume yields no false success and no duplicate accepted slot;
- concurrent writers publishing the same content converge on one verified object, and garbage collection never removes a referenced or leased object;
- archive and manifest tests cover traversal, escaping links, special files, decompression limits, undeclared files, and dataset mutation;
- plans distinguish declared from verified capabilities, and runtime capability or enforcement loss produces a typed failure;
- Nix package and app fixtures reject changes to any local flake input, lock, derivation, resolved program, output NAR, or recursive closure;
- exports retain invalid, censored, superseded, and failed attempts and refuse to silently pool incompatible timing, environment, or control classes.

### Explicit non-goals for v1

- inventing a universal package manager or lockfile;
- embedding language runtimes or transferring native language objects;
- replacing general workflow engines, BenchExec, or SLURM;
- distributed execution, hosted benchmark registries, or a remote multi-user service;
- embedded language models or provider-specific agent orchestration;
- publication-ready plotting, figure composition, or reporting;
- bitwise-identical performance across hardware;
- automatic adapters for arbitrary computational APIs;
- native Windows worker-handle transport in version 1.

The central invariant is:

> An attempt is accepted only when every declared dependency resolves to its expected typed identity, the canonical result satisfies the problem contract, all required controls were enforced over the selected timing scope, and the complete observation was durably published. Its environment and control guarantee classes remain explicit and independent of the language in which any worker is implemented.
