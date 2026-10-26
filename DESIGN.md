# Metewand: Reproducible, Language-Neutral Computational Benchmarks

## 1. Purpose

`metewand` benchmarks computational methods as implemented in **software**, not
merely their abstract algorithms. A benchmark may compare R packages, Python
packages, Julia packages, native executables, or any mixture of them without
privileging one host language.

The execution model covers bounded computations that map a configured dataset
and problem to a canonical result that can be evaluated independently. It also
covers bounded convergence profiles in which an implementation produces a
sequence of canonical checkpoint results and a problem-owned evaluator decides
whether a shared quality target has been reached. Optimization, estimation,
sampling, simulation, prediction, numerical integration, and similar scientific
computations fit this model. Long-running services, interactive systems,
request-throughput benchmarks, and distributed systems with no bounded result
artifact do not.

The central object is an immutable experiment composed of:

```text
configured dataset + configured problem + configured implementation
    + software environment + execution policy + observation policy
```

Datasets, problems, and implementations are parameterized definitions rather
than fixed objects. A dataset is a reproducible family of materialized data
artifacts; it may load recorded data, generate synthetic data, construct
simulation state, select a fold, or apply benchmark-defined preprocessing.
Expanding an experiment produces immutable dataset instances, problem instances,
implementation configurations, and, ultimately, run specifications.

The tool must make simple, single-language work pleasant while supporting
publication-grade runs with pinned datasets, pinned environments, controlled
execution, and complete provenance.

### Design principles

1. **One language-neutral orchestrator.** The main product is a standalone
   binary; it never embeds R, Python, or Julia.
2. **Processes, not FFI.** Workers communicate through a versioned protocol over
   pipes. No `rpy2`, PythonCall, RCall, or embedded interpreters.
3. **Native workflows remain native.** An R-only benchmark may use `renv`; a
   Python-only benchmark may use `uv`; a Nix-based benchmark may provide any or
   all worker environments from flakes. None requires another environment
   backend.
4. **Reproducibility is explicit.** Datasets, source files, lockfiles, images,
   and configurations are hashed or pinned.
5. **Parameter namespaces are explicit.** Dataset, problem, implementation,
   budget, and execution parameters are never conflated.
6. **Evaluation is independent of implementations.** Implementations return
   canonical results; the problem's benchmark-owned evaluator computes
   correctness and quality metrics in a separate worker.
7. **Strictness is graduated.** Local exploratory, locked, and isolated
   execution modes make distinct, limited guarantees.
8. **Automation is a first-class interface.** The CLI is deterministic,
   noninteractive, inspectable, and machine-readable so the same workflows
   compose cleanly in shells, CI, and agent loops.
9. **Applicability is explicit.** A configuration that an implementation cannot
   support is recorded as not applicable with structured evidence; it is never
   silently omitted or misreported as an execution failure.
10. **Profiles use common evaluation.** Implementations may expose different
    observation controls, but convergence targets are expressed in
    problem-owned metrics and saved results are evaluated independently of the
    implementations. Instrumented profiles may measure and stop locally, with
    their timing and stopping claims explicitly attributed to instrumentation.

## 2. User model

Authors define datasets, problems, implementations, and experiments. A problem
owns its scientific specification and independent evaluator. Its contract is
the normalized, versioned agreement about inputs, results, metrics, and
scientific rules; it need not be a separately authored component. The planned
compact form below lets authors describe that problem once, while Metewand
derives schemas and identities. A migration from a framework such as Benchopt
should reuse dataset, solver, and evaluation logic with small adapters.

A repository contains a manifest, optional native lockfiles, dataset
materializers, problem contracts and evaluators, implementation adapters,
schemas, and a generated lockfile. A small benchmark can keep declarations in
one manifest. A repository that shares external contracts and schemas can use
the following layout:

```text
benchmark/
├── metewand.yaml
├── metewand.lock
├── flake.nix                 # optional root-scoped Nix environment
├── flake.lock
├── datasets/
│   └── leukemia.py
├── problems/
│   ├── lasso.yaml
│   ├── lasso.py               # independent result evaluator
│   └── fixtures/
│       └── lasso-small/
│           ├── dataset/
│           ├── result/
│           └── metrics.json
├── implementations/
│   ├── glmnet.R
│   ├── sklearn.py
│   └── native.yaml
├── environments/
│   ├── r/
│   │   ├── renv.lock
│   │   └── renv/
│   └── python/
│       ├── pyproject.toml
│       └── uv.lock
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

The `datasets/` directory contains definitions that acquire, generate, or
transform data. The optional `assets/` directory contains repository-local
source files referenced by those definitions. Materialized dataset instances
live in Metewand's content-addressed cache, not in either directory. A problem's
evaluator is colocated with its contract because it is part of that problem,
although Metewand launches it as an independent worker in its declared
environment.

The `environments/` directory is a convention, not a semantic boundary.
Environment names are benchmark-defined and need not correspond to one language;
every backend-native project or lockfile is located through an explicit manifest
path. A self-contained Nix flake may therefore live under `environments/` like
any other definition. The example keeps `flake.nix` at the repository root
because that flake builds sources across the repository; its adjacent
`flake.lock` follows the flake root. Conda, OCI, and Nix environments may
provide several language runtimes or commands from one resolved software
context.

Benchmark manifests and problem contracts use YAML. Metewand-owned option keys
use kebab-case, such as `execution-policies` and `parameter-schema`. Enum values,
benchmark-defined names, parameter keys, and family-owned semantics retain their
declared spelling. Each document contains one mapping, and explicit keys are
unique strings. Anchors, aliases, and merge keys
are supported with bounded alias expansion. Explicit fields override merged
fields; earlier mappings in a merge sequence take precedence. Unsupported tags,
external includes, unsafe integers, and non-finite numbers are rejected. Literal
nulls remain available in canonical parameter values, while typed fields must
satisfy their declared shape. Native ecosystem files keep their own formats.

A representative manifest using the existing explicit declarations is:

```yaml
version: 1
name: "lasso-comparison"
problems:
  lasso:
    contract: "problems/lasso.yaml"
    evaluator:
      runner: "python"
      entrypoint: "problems/lasso.py"
      sources: ["problems/lasso.py"]
      environment: "python"
datasets:
  leukemia:
    parameter-schema: "schemas/leukemia-dataset-parameters.json"
    output-schema: "schemas/regression-dataset.json"
    runner: "python"
    entrypoint: "datasets/leukemia.py"
    sources: ["datasets/leukemia.py"]
    environment: "python"
    source:
      url: "https://example.org/leukemia.tar.zst"
      sha256: "..."
environments:
  r:
    kind: "renv"
    project: "environments/r"
    lockfile: "environments/r/renv.lock"
  python:
    kind: "uv"
    project: "environments/python"
    lockfile: "environments/python/uv.lock"
  native:
    kind: "nix"
    flake: "."
    installable: ".#native-lasso"
    system: "x86_64-linux"
    output-kind: "package"
    output: "out"
implementations:
  glmnet:
    runner: "r"
    entrypoint: "implementations/glmnet.R"
    sources: ["implementations/glmnet.R"]
    environment: "r"
    problem-contracts: ["lasso"]
    parameter-schema: "schemas/glmnet-parameters.json"
    capabilities: ["one_shot"]
  sklearn:
    runner: "python"
    entrypoint: "implementations/sklearn.py"
    sources: ["implementations/sklearn.py"]
    environment: "python"
    problem-contracts: ["lasso"]
    parameter-schema: "schemas/sklearn-parameters.json"
    capabilities: ["one_shot"]
  native:
    runner: "command"
    program: "bin/native-lasso"
    args: ["--metewand-worker"]
    environment: "native"
    problem-contracts: ["lasso"]
    capabilities: ["one_shot"]
experiments:
  - name: "main"
    problem: "lasso"
    datasets: ["leukemia"]
    implementations: ["glmnet", "sklearn", "native"]
    execution-policy: "controlled"
    observation-policy: "final"
    implementation-repetitions: 5
    measurement-repetitions: 2
    seed: 2025
    dataset-parameters:
      leukemia:
        fold: { grid: [1, 2, 3] }
        standardize: { value: true }
    problem-parameters:
      lambda: { grid: [0.001, 0.01, 0.1] }
      fit_intercept: { value: true }
    implementation-parameters:
      glmnet:
        tolerance: { value: 1e-7 }
      sklearn:
        selection: { grid: ["cyclic", "random"] }
execution-policies:
  controlled:
    version: 1
    cpus: 1
    threads: 1
    memory: "8 GiB"
    network: false
    worker-reuse: false
    warmup-runs: 0
    timeout: "10 min"
    timing-scope: "prepare_and_execute"
    primary-time: "timed_wall_time"
    run-order: "randomized"
    max-concurrency: 1
    enforcement: "best_effort"
observation-policies:
  final:
    version: 1
    kind: "one_shot"
```

The referenced problem contract is also concrete and reviewable. For example:

```yaml
version: 1
name: "lasso"
family: "optimization"
parameter-schema: "schemas/lasso-problem-parameters.json"
dataset-schemas: ["schemas/regression-dataset.json"]
result-schema: "schemas/lasso-result.json"
metric-schema: "schemas/lasso-metrics.json"
semantics-schema: "schemas/lasso-semantics.json"
allowed-timing-scopes: ["cold_end_to_end", "prepare_and_execute"]
supported-budgets: ["none"]
semantics:
  objective:
    family: "least_squares_l1"
    loss_scale: "1/(2*n)"
    penalty: "lambda*sum(abs(coefficients))"
    intercept_penalized: false
  data_conventions:
    standardization: "encoded_in_dataset_instance"
    intercept_column: "absent"
  preprocessing:
    allowed_outside_timed_scope: []
  validity:
    nonfinite: "reject"
  one_shot_completion:
    objective_relative_tolerance: 1e-8
    maximum_kkt_violation: 1e-6
reference-cases:
  - dataset: "problems/fixtures/lasso-small/dataset"
    result: "problems/fixtures/lasso-small/result"
    expected-metrics: "problems/fixtures/lasso-small/metrics.json"
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
metewand cache verify          verify exact cached tree identities
metewand cache roots           inspect durable named cache roots
metewand cache gc --dry-run    inspect collection from a committed catalog snapshot
metewand snapshot create       create a portable, selected result snapshot
metewand snapshot verify       verify a snapshot without importing it
metewand snapshot import       import a verified snapshot into the run store
```

### Authoring and scaffolding

`metewand init` creates a minimal, domain-neutral benchmark containing a
versioned manifest and no scientific definitions. It does not create datasets,
problem semantics, evaluators, fixtures, implementations, or software
environments.

Current component scaffolding is explicit and incremental.
`metewand scaffold dataset`, `metewand scaffold problem`, and
`metewand scaffold implementation` create only the structural files required
for the requested component and update the manifest accordingly. Generated
code may implement protocol and schema
boilerplate, but it must not choose problem semantics, validity criteria,
completion criteria, parameter meanings, reference results, or library-specific
translations. Those choices remain visible work for the benchmark author.

Scaffolding preserves YAML comments and formatting in block and flow mappings.
It refuses an anchored document root, an anchored or aliased target namespace,
merge keys in that namespace, or a namespace inherited through a root merge
before creating files. The diagnostic directs the author to expand the mapping
manually. Anchors on unrelated definitions and unrelated root merges remain
supported. The proposed manifest is reparsed to verify that only the requested
definition changes. Scaffolding validates the structural manifest and worker
declarations while allowing unfinished flat scientific definitions between
component additions. Full repository validation still requires those
definitions and author-supplied reference fixtures.

Metewand does not replace native environment initialization commands such as
`uv init` or `renv::init()`. Complete runnable benchmarks, including examples
such as a Python lasso comparison, are distributed under `examples/` rather than
exposed as privileged built-in templates.

The local MVP simplifies authoring around the Basin and clustlasso-cd
integrations. Scaffolds emit compact declarations in the manifest and small
SDK adapters in the selected Python, R, or Rust language. Reusable external
contracts and schemas remain supported. See the
[authoring reference](docs/reference/authoring-cli.md).

A problem contract and its schemas may be inline or referenced from reusable
repository files. Inline schemas use the same offline catalog and Draft
2020-12 dialect as referenced resources. Resolve references relative to the
owning document, normalize the effective declarations, and hash resolved
schema contents. External references and existing benchmark files remain
supported.

Result and metric envelopes may omit `schema`; the selected problem contract
provides the schema for validation. An explicit reference must still match the
selected schema. This permits workers to emit standard envelopes without knowing
an inline schema's internal catalog key.

Implementations, generated datasets, and problem contracts accept compact
`parameters` declarations with explicit types, constraints, optional fields, and
validated literal defaults or default grids. Contracts also accept compact
`result` and `metrics` declarations. Parsing generates normalized inline schemas
and literal defaults; declaration grids supply expansion axes without narrowing
the valid domain. Equivalent explicit inline schemas and literal defaults
retain canonical bytes and component identities. Plans expose resolved dataset,
problem, and implementation parameters for each candidate. The
[parameter reference](docs/reference/parameter-namespaces.md#compact-implementation-parameters)
defines the supported fields.

Omitting both `parameters` and `parameter-schema` produces the same closed
empty-object schema as `parameters: {}` for generated datasets, problem
contracts, and implementations. Unexpected parameters still fail validation.
An omitted worker environment selects the sole declared local environment. If
no environments are declared, Metewand inserts `local: {kind: local}` when a
worker needs it. Multiple local environments or exclusively managed environments
require explicit worker selections; Metewand never infers a managed backend.

Nested execution and observation policies inherit version 1 when omitted, and
both experiment repetition counts default to 1. Existing mechanical policy
defaults resolve before hashing and planning. Every selected execution policy
must declare `timing-scope` and `primary-time`; experiment policy selections and
the observation `kind` remain explicit. Preprocessing, validity, and completion
rules stay in the authored scientific contract. Plans expose effective parameter
mappings, environments, repetitions, and execution controls, and execution
retains the same resolved records.

#### Compact authoring

A problem's contract fields and evaluator may live directly under
`problems.<name>`. The flat form inherits contract version 1 and the mapping
name, normalizes through the same internal contract as a wrapped inline
mapping, and preserves equivalent identities. Explicit inline and referenced
contracts remain supported; standalone and wrapped contracts still require
`version` and `name`.

The following fragments show compact implementation parameters, declaration
grids, and a flat problem. Complete benchmarks also declare their
sources, environments, accepted datasets, scientific rules, reference cases,
and experiments.

```yaml
implementations:
  solver:
    runner: python
    entrypoint: implementations/solver.py
    parameters:
      tolerance:
        type: number
        exclusiveMinimum: 0
        default: 1e-6
      max_iterations:
        type: integer
        minimum: 1
        grid: [100, 1000]
problems:
  lasso:
    parameters:
      lambda:
        type: number
        minimum: 0
    result:
      coefficients:
        type: array
        items: { type: number }
      intercept: number
    metrics:
      objective: number
      kkt_violation: { type: number, minimum: 0 }
    evaluator:
      runner: python
      entrypoint: problems/evaluate.py
```

Keep `implementations.<name>.parameters` as a separate namespace because a
worker's launch settings and its scientific parameter names must not collide.
Use the same field declarations for generated-dataset and problem parameters.
A problem's parameters, `result`, `metrics`, scientific rules, reference cases,
and evaluator belong directly under `problems.<name>`. The author need not add
an inline `contract` wrapper. Metewand still constructs and hashes the internal
contract. Referenced contracts and supported explicit inline contracts remain
available; an author cannot combine a contract declaration with competing
inline problem fields.

The compact declarations follow these rules:

- Declare each field once. A type name such as `intercept: number` is shorthand
  for `intercept: { type: number }`. Start with scalar types, enums, and arrays,
  reusing JSON Schema constraint names such as `minimum`, `exclusiveMinimum`,
  and `items`. These constraint names retain their JSON Schema spelling within
  field declarations. Block and flow YAML mappings normalize identically.
- Fields are required and objects are closed by default, with an explicit way
  to mark individual fields optional. Never fabricate missing results or
  metrics. Full inline or referenced schemas remain available for complex or
  reusable boundaries. Compact and explicit forms may be chosen independently
  for parameters, results, and metrics; reject competing declarations of the
  same boundary or its parameter defaults.
- A parameter's type and bounds describe its valid domain. Its `default` is one
  literal value, while its `grid` is a default selection of values to benchmark.
  A field may declare either a default or a grid, but not both. A grid does not
  become an enum or define the full domain, and neither defaults nor grids
  supply inferred types or bounds. Results and metrics have no default or grid
  behavior.
- Lower compact defaults into the existing literal `parameter-defaults`
  mechanism and declaration grids into parameter expansion axes. Experiment
  axes replace declaration grids by parameter name; named cases replace
  experiment axes and inherit all other selections. Merge each expanded
  parameter object recursively with its literal defaults as in the existing
  resolution model. Defaults, grid members, and overrides must satisfy the
  declared domain. Preserve the existing Cartesian-product, coupled-case, and
  duplicate-candidate rules.
- Experiment and case parameter entries may use a plain scalar literal, such
  as `tolerance: 1e-6`. Retain `value` for literal arrays and objects and `grid`
  for sweeps, so an array value never becomes an implicit sweep. A missing
  required parameter with no effective value is an error.
- Normalize compact declarations before validation and identity construction.
  Equivalent compact and explicit declarations must produce the same effective
  schemas, parameters, and component identities. Resolve defaults before
  hashing configurations and show effective selections and settings in plans.
  Diagnostics identify the author's declaration or supplied value and its YAML
  location, rather than exposing a generated schema location.

Family semantics remain explicit and hashed, including preprocessing, validity,
and completion rules. Make a separate `semantics-schema` optional; authors may
supply one when machine validation of those rules is useful. Pass the resolved
scientific rules to the independent evaluator alongside the contract identity
and problem parameters. A tolerance declared in the problem should be consumed
by evaluator code rather than repeated as a hard-coded constant. Reference cases
continue to check scientific behavior. Metric declarations describe data shape;
they do not infer scientific formulas, validity, or completion thresholds.

Scaffolds emit the compact form and one SDK adapter per component. Keep the actual
scientific rules and adapter translations visible; defaults supply mechanical
settings. Authors choose a worker language with `--language python|r|rust`;
adapter callbacks fail with TODO diagnostics until the scientific decisions are
supplied. An omitted worker environment uses the same unambiguous local default
as other declarations. Native adapters are built in the author's Cargo project,
without scaffolding invoking a package manager or generating a native lockfile.
The complete comparison should need one manifest, small adapters, and scientific reference
fixtures, without hand-written protocol transport or artifact envelopes. New
public schemas should reuse authoritative boundary types where practical.
Rewriting the existing schema catalog is not a prerequisite for these
integrations.

#### Parameter expansion

Each experiment or case parameter entry is a plain scalar literal, an explicit
literal `value`, or an expansion `grid`. Plain scalars lower to `value` axes,
and declaration grids lower to the same expansion axes. This distinction permits
arrays and mappings to be literal parameter values. Grids form a Cartesian
product across the dataset, problem, and applicable implementation namespaces.

An experiment may instead contain named `cases`. Each case supplies a partial
set of dataset, problem, and implementation parameter values that is merged with
the experiment defaults before validation and expansion. Cases are unioned, not
multiplied, so coupled configurations such as `(n_samples, n_features,
density)` remain flat, reviewable parameter objects without requiring many
nearly identical experiments. Grids inside one case still form a Cartesian
product. The planner rejects duplicate logical candidates produced by
overlapping cases; statistical replication must use the explicit repetition
fields.

For example, these two cases select two coupled dataset shapes rather than four
independent grid combinations:

```yaml
experiments:
  cases:
    - name: "tall_dense"
      dataset-parameters:
        generated:
          n: { value: 10000 }
          p: { value: 100 }
          density: { value: 1.0 }
    - name: "wide_sparse"
      dataset-parameters:
        generated:
          n: { value: 1000 }
          p: { value: 10000 }
          density: { value: 0.01 }
```

### Schemas, canonical values, and identities

Version 1 uses JSON Schema 2020-12. Standalone schemas declare their dialect;
the inline form inherits it from the owning format. Every external
`$ref` must resolve to an admitted repository resource, recorded in the lockfile
when locked execution is selected; schema validation never retrieves a remote
reference. JSON Schema's `default` keyword inside an explicit schema remains an
annotation and is never inserted by Metewand. Compact parameter declarations
extract the authoring-level `default` into literal parameter defaults before
schema generation. Declaration-level `grid` is used only for expansion.
Neither becomes a new schema validation rule. A
definition may instead declare a literal
`parameter-defaults` object in the manifest;
defaults cannot contain grids. Defaults are merged recursively by object key,
while a supplied scalar, array, or `null` replaces the default at that key.
Metewand performs this merge before validation, records the resolved object, and
hashes only the resolved object.

Parameter values use a language-neutral JSON subset. Integers lie in
`[-(2^53 - 1), 2^53 - 1]`; noninteger numbers are finite IEEE-754 binary64
values; negative zero is normalized to zero; and duplicate object keys are
rejected. Strings are Unicode scalar values. Version 1 canonicalization follows
RFC 8785 for this restricted domain. Schema validation occurs before
canonicalization, and the exact canonical bytes are used for hashing and
transport. The repository contains golden canonicalization vectors that every
SDK must pass.

Every stable identity is a typed, versioned content identity:

```text
mw1-<kind>-<hex SHA-256(kind || NUL || canonical representation)>
```

The canonical representation refers to dependencies by their typed identities,
forming a Merkle graph. It never contains workspace, staging, or cache paths,
timestamps, or map iteration order. Backend-native immutable addresses, such as
a locked Nix store program, may appear alongside their content hashes.
Definition identities include their schemas, contracts, declared source bundles,
and unresolved environment definitions. Resolved identities additionally include
materialized artifact hashes, the resolved environments and launch programs for
every worker role, applicability decisions, observation policies and controls,
the executor definition, the wire-protocol version, and a Metewand
execution-semantics version. The exact Metewand build remains attempt provenance;
a behavior change that can affect execution increments the semantics version.

The YAML migration retains the unreleased version-1 typed manifest,
public-schema, identity, and wire contracts and version-2 execution semantics
after explicit compatibility review. The subsequent kebab-case option-key change
also retains these versions: typed canonical records, identities, wire messages,
and execution behavior keep their existing representations. Underscore option
keys are rejected, and the public manifest and contract schemas describe the
new spelling. Alias resolution and equivalent merge expansions do not
change canonical values. Referenced resource renames from `.toml` to `.yaml`
can change hashes or identities that contain those paths. Existing run-store
records remain readable.

The whole-manifest hash is computed from the versioned typed manifest after
built-in defaults, path normalization, and name resolution, not from YAML
spelling or comments. In `metewand.lock` it proves that the lock belongs to the
current manifest, but it is not a dependency of each component identity.
Consequently, adding an unrelated experiment invalidates the lockfile until it
is regenerated without changing the identities of unaffected configurations or
runs. Identity rules and golden vectors are part of the public compatibility
contract.

Every interpreted worker declares `sources`, a list of repository-relative files
or glob patterns that form its source bundle. Patterns are expanded in sorted
bytewise path order, must match at least one entry, may not escape the
repository through a path or symlink, and are hashed with the tree rules used
for artifacts. Imported helpers, configuration files, and other runtime-read
repository files must be included. Locked isolated executors expose only the
declared bundle; weaker executors verify it before and after execution and
record that undeclared-file access could not be prevented. Native code contained
entirely in a resolved environment closure does not require a separate source
bundle.

Parameter ownership is semantic rather than inherited from a library API:

- dataset parameters determine acquired, generated, selected, or transformed
  dataset artifacts, such as recorded data, simulation state, sample size,
  noise, preprocessing, or a fold;
- problem parameters determine the computation and result semantics, such as an
  estimand, loss, regularization strength, prediction target, integration
  domain, or simulation horizon;
- implementation parameters determine how one implementation executes that
  problem, such as tolerance, method variant, approximation, or internal
  initialization;
- budgets and execution parameters determine the comparison conditions without
  changing the problem or implementation configuration.

Every selected implementation configuration must execute the same problem
instance. A library option that changes the requested computation, dataset
transformation, target, or result interpretation is therefore a dataset or
problem parameter even if the library presents it as an implementation option.
Metewand validates declared schemas and namespaces, but semantic classification
remains the benchmark author's responsibility and is reviewable in the problem
contract.

The problem contract is a required, structured, hashed artifact rather than an
integer label or evaluator convention. Its common envelope identifies the
requested computation, accepted dataset schemas, parameter and canonical result
schemas, metric schema, and supported timing scopes and budgets. The problem
definition owns an independent evaluator worker, including its source bundle and
environment; it is not a separately selectable benchmark component. The `family`
value is an open, stable tag rather than a closed Metewand enumeration. A
family-specific `semantics` object binds the remaining rules. It requires explicit
`preprocessing`, `validity`, and `one_shot_completion` mappings. An optional
`semantics-schema` adds family-specific validation. Metewand hashes the complete
object but does not need to understand its scientific fields. For example, an optimization contract binds its
objective, constraints, scaling, preprocessing, result-validity rules, and
one-shot completion tolerances; other problem families bind their own
domain-specific semantics. A contract may link explanatory prose, but every rule
that affects execution, observation validity, or completion is
represented in its validated structure. Each problem ships small reference
instances and expected metrics that `metewand check --workers` evaluates as
conformance tests.

Schema identifiers and contents are versioned and hashed. Before materialization
or execution, the planner requires each dataset definition's output schema to
match a schema accepted by the selected problem contract. Cross-schema
conversion requires an explicit dataset materializer; it is never inferred from
field names or shapes.

A problem may declare `dataset-schemas: []` when it requires no external
dataset. Such an experiment must omit `datasets`; the planner binds one
versioned, built-in unit dataset definition, configuration, and instance rather
than producing zero runs or requiring a dummy user file. A problem that requires
a dataset must select at least one dataset definition. Each selected definition
expands into separate problem instances, each of which binds exactly one dataset
instance. Problems that need several logical data components represent them in
one structured dataset instance with a contract-owned schema.

Every implementation definition declares the problem contracts it implements.
Manifest names are resolved to exact contract identities during planning, and
that resolved set is part of the implementation-definition identity. The planner
rejects any implementation/problem pairing outside the set; `prepare` receives
the selected contract identity, and contract conformance fixtures test that
declaration against the actual worker.

An implementation may additionally declare the `applicability` capability when
support depends on resolved problem parameters or materialized dataset
properties. Before allocating a measured attempt, Metewand asks the worker to
decide applicability from the immutable dataset instance, its manifest, the
problem contract and parameters, and the implementation parameters. The response
is either `applicable` or `not_applicable` with a stable code, a human-readable
reason, and optional structured details. Absence of this capability asserts that
the implementation is applicable to every configuration admitted by its
declared problem contracts and schemas.

Applicability checks run outside every performance timing scope and may inspect
but not mutate the private dataset view. Their worker definition, environment,
inputs, response, and observed controls form a typed, hashed decision record. A
not-applicable candidate produces no measured attempt slots, remains visible in
status and exports, and is not a failure. In locked mode the decision is recorded
during resolution and must be reproduced before execution. This mechanism is
for intrinsic support constraints, such as unsupported intercepts, sparse
matrices, or unsafe problem dimensions; using it to suppress slow or
unfavorable results is a reviewable benchmark-definition error.

The experiment seed deterministically derives separately recorded dataset,
implementation, and scheduling seeds using
`SHA-256("metewand-seed-v1" || NUL || role || NUL || canonical seed fields)`.
The dataset-seed fields are the experiment seed, dataset-definition identity,
and resolved dataset parameters. The implementation-seed fields are the
experiment seed, dataset- and problem-configuration identities, and
implementation-replication index; they deliberately exclude the implementation
identity. The scheduling-seed fields are the experiment seed, benchmark and
experiment names, and scheduling-policy version, but not the expanded member
set. The first 53 digest bits are exposed to workers as a nonnegative JSON
integer, and the complete derivation digest is recorded. No derivation depends
on expansion order. Implementations in the same problem-instance and
implementation-replication block therefore receive the same numeric seed,
although Metewand does not claim that different random-number generators produce
paired random streams.

The role is exactly `dataset`, `implementation`, or `scheduling`. The canonical
seed fields are the following restricted-domain JSON objects; identity values
use their complete typed `mw1-<kind>-<sha256>` spelling, and
`dataset-parameters` embeds the already resolved canonical value.

```text
dataset = {
  "dataset_definition": <dataset-definition identity>,
  "dataset_parameters": <resolved canonical parameters>,
  "experiment_seed": <safe integer>
}

implementation = {
  "dataset_configuration": <dataset-configuration identity>,
  "experiment_seed": <safe integer>,
  "implementation_repetition": <safe integer>,
  "problem_configuration": <problem-configuration identity>
}

scheduling = {
  "benchmark_name": <name>,
  "experiment_name": <name>,
  "experiment_seed": <safe integer>,
  "scheduling_policy_version": <safe integer>
}
```

`SEED_DERIVATION_VERSION` and `SCHEDULING_POLICY_VERSION` are both `1`. The
former selects the `metewand-seed-v1` transcript; the latter is an explicit
scheduling-seed field. Interpreting the digest as a big-endian bit string, the
exposed value is bits 0 through 52, equivalently the leading big-endian `u64`
shifted right by 11.

`implementation-repetitions` creates distinct logical run specifications by
varying the implementation seed. `measurement-repetitions` repeats the selected
observation policy for the same logical specification and seed. Each one-shot
repetition creates one observation and attempt slot; each profile repetition
creates its own bounded observation slots and one or more execution slots
according to the selected delivery mode. Resolution later binds each slot to a
resolved specification. Dataset replication is expressed through an explicit
parameter such as `replicate`, which participates in the dataset seed. Version 1
has no hidden problem seed: random quantities that define the requested
computation must appear explicitly in canonical problem parameters or be
materialized into the dataset instance. A problem `replicate` value is therefore
an ordinary explicit parameter interpreted by the contract. Deterministic
scheduling assigns pseudorandom block and within-block priorities derived from
the scheduling seed and typed block or slot identity, then sorts by those
priorities. Adding an unrelated slot therefore does not reorder existing slots
relative to one another.

The version-1 attempt-slot priority transcript is:

```text
SHA-256("metewand-scheduling-priority-v1" || NUL ||
        scheduling-seed digest || NUL || role || NUL || anchor)
```

The scheduling-seed digest is its raw 32 bytes. The measured anchor is the
complete typed logical-observation-slot identity. The warm-up anchor is the
complete typed logical-specification identity followed by `NUL` and the warm-up
index as an eight-byte big-endian integer. The role is exactly `measured` or
`warmup`. This priority is attached during logical planning; later scheduling
composes it with the comparison-block priority.

The version-1 comparison-block priority uses the same transcript with role
`block`. Its anchor concatenates the complete typed dataset-instance identity,
`NUL`, the complete typed problem-configuration identity, `NUL`, the
implementation-repetition index, `NUL`, and the measurement index. Both indices
use eight-byte big-endian encoding. The scheduler sorts by block priority, then
the typed block fields in that order, then attempt-slot priority, then logical
attempt-slot identity, all in ascending byte or numeric order. The identity
tie-breakers make equal priorities independent of expansion order. Experiments
retain their declared order. Sequential scheduling retains logical expansion
order, and worker-lifecycle warm-ups run immediately before the measurement
that requires them rather than participating in the comparison sort.

## 3. Core architecture

Metewand is a Rust workspace with one distributable executable and small
optional language SDKs:

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
│   └── julia/
├── schemas/
├── examples/
│   ├── r-only/
│   ├── python-only/
│   ├── nix-locked/
│   └── mixed/
└── docs/                     # VitePress website
    ├── guide/                # introductory guides
    └── reference/            # implemented interfaces and contracts
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
- monotonic timing around the implementation call;
- bounded local trace collection and dispatch to benchmark-supplied observers.

Any executable may implement the protocol directly; an SDK is never required.

### Automation and agentic workflows

The command-line interface is the universal automation API. It must work without
a terminal, browser, editor integration, or model-specific plugin. Claude,
Codex, other agents, CI jobs, and ordinary scripts receive the same semantics;
an optional MCP adapter may later expose these operations without creating a
second behavioral API.

Every command supports a versioned machine-readable output mode. In that mode,
standard output contains only JSON or JSON Lines conforming to published
schemas; progress and human diagnostics go to standard error. Exit codes and
diagnostic codes are stable and documented. Structured diagnostics include
source spans, affected stable identities, causal chains, and concrete
remediation where it can be stated safely. Human-readable output is rendered
from the same typed records rather than maintained as a separate source of
truth. SDK callback failures include bounded exception text in their existing
version-1 failure messages, so failed attempt diagnostics remain inspectable
through `status` and `explain` after private worker directories are removed.
Python includes traceback context, R includes the condition message, and native
Rust workers include string panic payloads when unwinding is enabled. Truncated
messages carry an explicit marker.

Commands are noninteractive by default and never silently rewrite manifests,
native lockfiles, or source files. An operation that requires an additional
mutation or capability fails with an explanation and the explicit flag or
command that authorizes it. Destructive cleanup is a separate command with a
dry-run mode and exact artifact targets.

The authoring commands are the deliberate exception to the default read-only
posture. `init` and `scaffold` report every path they create or modify, support
a dry-run mode, and fail rather than overwrite an existing file. Scaffolding may
edit the manifest only as part of the component addition explicitly named by the
user; it never invokes an ecosystem package manager or modifies a native
lockfile.

The read-only `plan` command performs schema validation, deterministic
expansion, declared-capability compatibility checks, and logical candidate-ID
assignment without downloads, builds, worker launches, or filesystem mutation.
Worker and executor capabilities in a plan are explicitly labeled `declared`,
not verified. Configuration-dependent applicability that requires a materialized
dataset or worker call is labeled `unchecked`; the plan reports both the
candidate count and the number of attempt slots whose applicability is already
known. `check --workers`, `lock`, and `run` may launch workers, resolve
applicability, and preflight executors. They require every declared capability
used by an applicable candidate to be observed and label it `verified_now` or
fail. Additional observed capabilities are recorded but not selected
implicitly. A lock records the complete capabilities and applicability
decisions observed during resolution as `previously_verified`, and `run`
verifies the selected subset and decisions again before use. This distinction
prevents a side-effect-free plan from claiming knowledge that requires
execution.

Resolution through `lock` adds artifact and environment fingerprints and
produces a separate resolved execution ID. This distinction keeps plans
inspectable before expensive provisioning without pretending that an unresolved
environment is known. Its output includes:

- every dataset, problem, implementation, logical candidate, applicable run,
  observation, and logical attempt-slot identity;
- the number and order of candidate combinations, applicable runs, observation
  slots, and execution slots;
- required artifacts, environment resolutions, executors, and controls;
- anticipated network access, builds, commands, and writable paths;
- checked and unchecked applicability, structured reasons for known exclusions,
  and unsupported capabilities or blockers, with each capability's verification
  state.

Long-running operations emit append-only structured events. An authoritative
SQLite database in WAL mode with full synchronous durability coordinates
applicability decisions, resolved attempt and observation slots, leases,
transitions, durable checkpoint observations, and finalized artifact references;
event rows are insert-only, and state changes occur in transactions. The
[version-1 SQLite schema](docs/reference/run-store-schema.md) defines this coordination
layout. The runtime initializes stores with verified WAL/FULL settings and
filesystem capability probes. Its transactional writer API fences reservations,
transitions, finalization claims, and terminal outcomes by lease ownership and
revision. Its publisher creates leased same-filesystem stages, validates and
synchronizes complete artifacts, publishes them without replacement, and only
then commits their references and terminal state. Explicit recovery validates
completed orphans, durably fails interrupted attempts, and quarantines abandoned
stages while preserving claims until terminal commit. The CLI uses this store
for selected and bounded execution, explicit retries, and resume. Uniqueness constraints permit at most one
accepted attempt per resolved execution slot and one selected valid observation
per resolved observation slot.
Before writing its immutable outcome record, a successful attempt atomically
acquires a leased finalization claim; a concurrent loser is finalized as
superseded. The claim alone never satisfies the slot: acceptance becomes
terminal only after durable artifact publication and the final database
transaction. Recovery resolves an expired claim from its completed directory or
marks the interrupted attempt failed before another retry can claim the
slot. The run store checks required locking, synchronization, and atomic-rename
behavior at initialization and refuses durable mode on an unsupported
filesystem. Immutable attempt directories remain self-describing, so the
database and export indexes can be rebuilt from finalized artifacts.
`run --resume` fills unsatisfied resolved slots without replacing selected valid
observations or rerunning satisfied one-shot or fresh-sequence execution slots.
It does not claim to restore opaque implementation state: unless an
implementation later declares a contracted checkpoint-restore capability, a
failed streaming attempt restarts from clean state and may recompute earlier
checkpoint indices. Those checkpoints remain associated with their existing
observation slots and cannot overwrite selected observations. A failed slot is
retried only with an explicit retry policy or flag, and every retry remains a
distinct `RunAttempt`. Interrupted staging directories carry leases and are
either safely resumed by their owner or quarantined after the lease expires.
Filters operate on stable logical or resolved candidate, run, observation,
slot, attempt, or applicability identities, and explicit bounds such as maximum
attempts or selected IDs let an agent test a small change before expanding to
the complete benchmark.

This interface is agent-friendly, not agent-dependent. Metewand does not embed a
model, send source or results to an external service, generate scientific
conclusions, or grant an agent authority beyond the invoked command. All
operations remain usable and auditable as ordinary local CLI calls.

The read-only `status` and `explain <id>` commands inspect checked logical plans,
committed run-store history, or both through explicit `--manifest` and
`--output-dir` selection. Status retains every retry, superseded attempt, and
selected observation, distinguishes pending work from expired reservations,
and labels historical slots absent from the current plan. Explanation follows
exact stable identities across definitions, runs, slots, attempts, artifacts,
and persisted failure diagnostics. The inspection reader uses a private SQLite
snapshot without initializing or recovering the source store. Human and machine
output share core inspection records and the version-1 `inspection-output`
schema. See [Status and identity explanations](docs/reference/inspection-cli.md).

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
Environment                 reproducible software context for one or more workers
ResolvedEnvironment         resolved context, fingerprint, and launch capabilities
Runner                      launch convention for an SDK or raw command worker
ResolvedLaunchSpecification exact executable, arguments, variables, and working directory
Executor                    mechanism that launches and constrains a worker
ExecutionPolicy             resources, isolation, timing policy, and repetition policy
ObservationPolicy           one-shot or profile kind, controls, target metric, and bounds
LogicalCandidateSpecification
                      dataset + problem + implementation configurations + policies
ApplicabilityDecision candidate + evidence + applicable or not-applicable outcome
LogicalRunSpecification
                      applicable candidate + optional scientific budget
                      + implementation-replication index + implementation seed
ResolvedRunSpecification
                      logical specification + materialized dataset instance + resolved source,
                      environment, launch, problem evaluator, and executor artifacts
LogicalObservationSlot
                      logical specification + observation control or snapshot index
                      + measurement index
ResolvedObservationSlot logical observation slot + resolved specification
LogicalAttemptSlot    one-shot, fresh-observation, stream, or buffered execution + warm-up role
                      + schedule priority
ResolvedAttemptSlot   logical slot + resolved specification
RunAttempt            resolved slot + retry identity + one observed execution
RunObservation        resolved observation slot + producing attempt + canonical result,
                      evaluator metrics, observation timing, and measurement basis
ResultSnapshot        selected attempts and observations + artifacts + export lineage
```

Definitions describe parameterized families, and configurations bind canonical
parameter values. Logical candidate identities can therefore be assigned before
acquisition or environment provisioning even when applicability is still
unchecked. Potential logical run, observation, and execution-slot identities
are deterministic from the candidate, but Metewand admits them to the run plan
only after the candidate is found applicable. Resolution binds immutable
artifacts, environment fingerprints, exact launch specifications, and
applicability evidence into corresponding resolved identities containing every
declared dependency needed for execution. A `ResolvedAttemptSlot` represents
one planned execution under one resolved dependency set; it has at most one
accepted attempt but may retain multiple failed or superseded retries. A
`RunAttempt` adds observed controls, machine context, state, timestamps, and an
outcome without changing the logical or resolved identities. Each independently
evaluated result is a `RunObservation`; a one-shot attempt has exactly one,
while a profile attempt may produce several durable observations before it
terminates.

`ImplementationDefinition` and `Environment` remain deliberately separate. The
same adapter may run locally during development and in an OCI image for an
archival run. An environment is not identified with a language or a single
executable: one resolved Conda prefix, Nix output, or OCI image may launch
workers through different runners. Each resolved launch remains a separate
dependency of the run identity.

### Problem and fairness contracts

A problem definition owns a versioned contract and its executable evaluator.
The manifest may inline the contract or reference a reusable YAML file.
Together, they bind:

- accepted dataset and problem-parameter schemas;
- canonical result and metric schemas;
- the evaluator worker and its environment;
- the requested computation and domain-specific conventions;
- supported budget types and their semantics;
- correctness tolerances and rules for invalid results.

The contract remains language-neutral and reviewable, while its evaluator may be
implemented in any supported language and runs through the worker protocol. This
is a runtime boundary, not another experiment axis: an evaluator is owned by
exactly one problem definition and cannot be selected independently.

Every experiment resolves a versioned execution policy. The policy specifies
worker reuse, permitted caching, warm-up runs, run ordering, concurrency,
timeouts, resource requirements, and one timing scope:

- `cold_end_to_end`: time from immediately before worker launch through receipt
  of the complete canonical result;
- `prepare_and_execute`: launch and handshake first, then time `prepare`,
  `execute`, and canonical result writing;
- `execute_only`: time only `execute` and canonical result writing; permitted
  only when the problem contract precisely defines allowed untimed preparation.

The authoritative `timed_wall_time` is measured by the orchestrator around the
complete selected scope. The policy's scientific `timeout` applies to that same
scope. Materialization, untimed startup or preparation, evaluation, reset, and
shutdown have separate operational timeouts with fully resolved defaults, so no
worker phase can hang indefinitely. Executor-observed CPU time may be another
primary measure only when it covers the same process tree and scope. In-worker
`implementation_time` is secondary diagnostic provenance; it cannot replace
the execution policy's primary measure. An explicitly selected instrumented
profile may instead use local measurements for its curve coordinates under the
rules below, while retaining the executor-observed attempt duration.
Metewand-owned validation and evaluation occur after the timed scope. Dataset
integrity checks surround worker operations. For `prepare_and_execute`, the
timer remains continuous from immediately before `prepare` through receipt of
the complete `execute` response.
Integrity checks between those operations therefore contribute to the measured
duration, and execution provenance records their inclusion. Compilation,
preprocessing, conversion, and result serialization are included whenever they
occur inside the selected scope, and the run record identifies excluded phases
explicitly.

Exports label `execute_only` observations as operation-scope benchmarks rather
than end-to-end software measurements, and compatibility readers never pool
different timing scopes implicitly.

`cold_end_to_end` requires `worker-reuse: false` and `warmup-runs: 0`, because
every measured slot must launch a fresh process. Process-local warm-ups require
worker reuse and execute on the same worker before its measured slots. Version 1
rejects `warmup_runs > 0` with `worker-reuse: false`; system-cache or thermal
conditioning must instead be an explicitly named later policy feature. A
discarded worker's warm-ups never satisfy a replacement worker. These rules make
the timing claim mechanically observable, while the independent evaluator makes
the result checkable.

When omitted, the version-1 timing fields default to `worker-reuse: false`,
`warmup-runs: 0`, and `timing-scope: "prepare_and_execute"`. Compatibility
validation applies these defaults before checking the invariants above and the
problem contract's allowed timing scopes.

Every experiment also resolves an observation policy. A `one_shot` policy
produces one canonical result per run under an optional problem-defined
scientific budget. A `profile` policy produces a time-quality curve; each
selected implementation binds a delivery mode under the same policy:

- `fresh_sequence` executes a bounded sequence of implementation-specific
  observation controls from clean algorithmic state and independently evaluates
  each result;
- `streaming_profile` executes once and cooperatively publishes bounded
  checkpoint results for independent evaluation while preserving algorithmic
  state between checkpoints;
- `buffered_profile` executes once, records a bounded local trace and selected
  result snapshots, and returns them for independent evaluation after execution.
  It supports direct recording and a local measurement-and-stopping callback.

Fresh-sequence and buffered profiles are required by the local MVP; streaming
profiles follow it. Their examples below
specify intended interfaces, not capabilities of the current CLI or SDKs.
Implement each mode with its actual producer, consumer, and focused behavioral
tests. Apply the [pre-MVP compatibility policy](docs/reference/compatibility-versions.md)
only to affected boundaries. This design change does not change current
constants or accepted inputs; strict schemas must agree with the implemented
readers when the features arrive.

For example, one profile may bind heterogeneous SLOPE adapters without treating
their controls as a common budget:

```yaml
observation-policies:
  target_gap:
    version: 1
    kind: "profile"
    metric: "relative_duality_gap"
    direction: "at_most"
    threshold: 1e-7
    max_observations: 100
    max_active_time: "30 s"
    implementations:
      iterative:
        delivery: "fresh_sequence"
        control_schema: "schemas/iteration-control.json"
        schedule: { kind: "linear", field: "iterations", start: 1, step: 1, count: 100 }
      tolerance:
        delivery: "fresh_sequence"
        control_schema: "schemas/tolerance-control.json"
        schedule: { kind: "geometric", field: "tolerance", start: 0.1, ratio: 0.1, count: 12 }
      callback:
        delivery: "streaming_profile"
        control_schema: "schemas/callback-progress.json"
```

An observation policy may run a fixed number of observations or stop when a
named evaluator metric reaches a threshold. The metric, direction, threshold,
maximum observation count, and time bound are canonical policy fields. The
metric name must exist in the problem's metric schema. The evaluator, not the
implementation, decides whether a saved result has reached the target. Streaming
and fresh-sequence profiles use that decision during execution; buffered
profiles validate local stopping claims afterward. Later planned
observation and execution slots that are no longer needed receive the terminal
disposition `skipped_target_reached` rather than disappearing or masquerading as
observed data.

Scientific budgets and observation controls are distinct. Evaluation, sample,
or domain-specific budgets are permitted only when the problem contract defines
their shared semantics and the worker declares the corresponding capability.
An observation control merely asks one implementation for an operating point,
such as an iteration limit or library tolerance. Each implementation bound
through `fresh_sequence` declares a separate observation-control schema and a
finite explicit, linear, or geometric schedule in the manifest. Controls and
their schedules are hashed and recorded, but their numeric values are not
presumed comparable across implementations. Comparison is based on common
evaluator metrics and an explicitly identified timing source: executor-observed
by default, or instrumented under the opt-in rules below.

The problem contract distinguishes result validity from profile completion. A
schema-valid, finite, semantically meaningful early checkpoint may be retained
as a valid observation even though it has not reached the policy's quality
target. Invalid or infeasible results terminate the producing attempt under the
contract's rules. A one-shot policy may require a completion threshold before
accepting its attempt; a profile policy uses its explicit target and bound.

For `fresh_sequence`, every observation control receives clean implementation
state; worker-process reuse and process-local warm-up remain governed by the
execution policy. For `streaming_profile`, the worker pauses after publishing a
checkpoint until Metewand validates and evaluates it and replies with
`continue` or `stop`. Checkpoint result writing is included in active execution
time, while independent evaluation is excluded and recorded separately. The
orchestrator records cumulative executor-observed active wall and process-tree
CPU time at every checkpoint as well as total elapsed wall time. Scientific
timeouts apply to active timed execution; separate operational limits bound
checkpoint evaluation and total session lifetime.

Work-unit counts are therefore useful controls and diagnostics, not an assumed
fair comparison scale. A benchmark that does not request a profile compares
final results under explicit implementation completion or stopping rules.

### Instrumented profiles and local observers

Internal solver comparisons should not require a fresh solve for each accuracy
target or an evaluation round trip at every diagnostic check. A
`buffered_profile` preserves algorithmic state in one execution and separates
local observation from later independent validation. The implementation can
record observations directly, or invoke a local observer from its iteration
loop. Both interfaces produce the same buffered artifacts and use the same
negotiated worker capability; a callback is not a separate transport mode.

The profile binding explicitly selects `measurement_basis: "instrumented"`
and a contract-defined `instrumented_scope`. Problem contracts must opt in to
instrumented measurements and define each permitted scope's clock origin,
included work, and excluded preparation. Contracts that omit this permission
allow only executor-observed profile measurements. For example:

```yaml
observation-policies:
  internal_gap:
    version: 1
    kind: "profile"
    metric: "relative_duality_gap"
    direction: "at_most"
    threshold: 1e-8
    record_thresholds: [1e-4, 1e-6, 1e-8]
    max_observations: 3
    max_trace_points: 10000
    max_buffer_bytes: 67108864
    max_active_time: "30 s"
    implementations:
      hybrid:
        delivery: "buffered_profile"
        measurement_basis: "instrumented"
        instrumented_scope: "complete_solver_call"
        instrumentation_schema: "schemas/hybrid-observer.json"
        instrumentation: { check_every: 5, observer: "duality_gap" }
```

The optional `record_thresholds` list requests snapshots at the first locally
observed crossing of each listed threshold. Values are finite, distinct, ordered
from least to most demanding under the metric direction, and end at the
profile's completion threshold. A snapshot may serve several thresholds crossed
at one check; it is evaluated once and retains all corresponding threshold
references. The policy must permit enough observations for the requested
snapshots, including the last observed result if execution stops short of the
target. Thresholds do not request independent solves. A study that needs
different stopping settings to change the trajectory must declare separate run
specifications rather than interpreting one trace as those separate solves.

Instrumentation configuration includes the observer selection, check schedule,
snapshot rule, and any locally applied bounds. It is schema-validated, hashed,
and passed to the worker. Observer code and its dependencies belong to the
declared worker source bundle and environment; the problem's independent
evaluator retains its own source bundle and environment. Changing an observer,
schedule, timing scope, measurement basis, or threshold list changes dependent
run identities. No observer code is loaded into the orchestrator.
Buffered snapshot slots use their bounded capture ordinal, not measured times or
metric values, with threshold and trace references recorded as observation
metadata. Direct and callback recording of the same selected instrumentation
must follow the same slot rules.

The local observer is an ordinary function called synchronously by the solver:

```text
observe(progress, measure, snapshot) -> continue | stop(reason)
```

`progress` contains a monotone check index and implementation-specific work
counts. `measure` computes problem-defined provisional metrics at the current
iterate. The observer invokes it at the declared checks, reads a monotonic clock
after measurement, records the metrics and cumulative elapsed time, and returns
a decision under the selected policy. `snapshot` lazily captures an immutable
canonical result when a threshold is crossed or the last observed result must
be retained. It must capture that checked iterate before the solver mutates it.
The callback does not expose a live solver object across the process boundary,
perform protocol I/O, or invoke the independent evaluator.
Measurement and snapshot functions read the current iterate without changing
algorithmic state, random-number streams, or update schedules. The observer may
stop execution but cannot steer subsequent updates; numerical diagnostics may
use separate scratch storage.

At solver termination, the observer performs a final check if the current
iterate has not already been checked and retains that result if needed. The
policy reserves capacity for this terminal observation; the same metric and
snapshot costs apply. No retrospective timestamp is assigned to an unchecked
final state. A solver that stops between scheduled checks records this terminal
check separately from the regular schedule.

The solver must honor `stop` before another update and finalize its buffered
results; continuing is a protocol-contract violation. The decision records
whether the local target, a declared bound, or an implementation stopping rule
caused termination. A provisional metric can trigger local stopping but cannot
establish accepted target attainment. Direct recording must preserve the same
clock, snapshot, bound, and stopping semantics. Existing solver diagnostics may
provide the measurement so the observer need not repeat the numerical work.

Clock reads, metric computation, callback dispatch, and earlier snapshot work
remain in cumulative instrumented time; the clock never pauses to subtract
instrumentation costs. A point's timestamp follows its metric computation and
precedes copying that point's snapshot. Its snapshot cost therefore enters
subsequent points and total attempt time, while final serialization enters the
executor-observed duration. This boundary is part of the named scope, not an
implicit end-to-end measurement. Internal stopping checks and traces remain
solver work. An observer does not require callback crossings into a host
language: a Rust loop may implement the same local API natively and return the
buffer after the solve.

The worker returns a bounded trace of check indices, work counts, provisional
metrics, and nonnegative, nondecreasing integer nanosecond offsets, together
with selected immutable result snapshots, their trace references, and its stop
reason. A trace row without a saved canonical result is diagnostic provenance,
not an independently validated `RunObservation`. Count, byte, and active-time
bounds apply during collection, not just at upload. Capacity for the terminal
trace entry and result is reserved. Reaching a collection bound stops the solve
explicitly; a result too large to fit produces a typed resource-limit failure,
and entries are never silently dropped. The executor still enforces the
scientific timeout over the selected execution scope,
including snapshot copying and serialization, regardless of the worker's local
clock or decisions.

After execution, Metewand validates the complete buffer and independently
evaluates each saved result. Evaluator metrics determine saved-result validity
and accepted target attainment. Provisional metrics remain separately named;
the contract defines any required agreement tolerances and mismatch failures.
If a locally claimed target is not confirmed, a valid result remains available
but the attempt is censored unless a contract violation requires failure. Early
termination for another solver rule or bound is also censored when the evaluator
has not confirmed the target. Fixed-count completion requires its declared
observations. Independent evaluation time and total experiment wall time remain
visible even though evaluation occurs outside the timed solve.

Records and exports distinguish the following evidence:

  | Claim                                                            | Evidence                                  |
  | ---------------------------------------------------------------- | ----------------------------------------- |
  | Total selected execution duration                                | Executor-observed timing                  |
  | Time at an internal check                                        | Worker-reported monotonic instrumentation |
  | Quality of a saved iterate                                       | Independent evaluator metrics             |
  | Quality at an unsaved check or first observed threshold crossing | Local instrumentation only                |

An instrumented curve may be the primary analysis for an explicitly instrumented
experiment. Independent evaluation does not verify its timestamps or prove
that an earlier unsaved iterate failed the target. Exports label its points as
instrumented, independently validated only where a snapshot exists, and first
observed rather than exact first-hitting times. Readers never silently pool
instrumented curves with executor-observed curves or different instrumented
scopes. Heterogeneous delivery modes can share a scientific target without
acquiring equivalent timing evidence.

Buffering trades immediate durable checkpoints for lower observation overhead.
A worker crash before a complete buffer is returned provides no accepted trace
points. After a valid buffer is received, each independently evaluated snapshot
is durably published in order and survives later evaluation or publication
failure. Retries start clean algorithmic state and never splice partial curves
from separate executions. Streaming profiles remain the mode for evaluator-owned
online stopping and durable progress during a long solve.

## 4. Worker protocol

Use a versioned JSON-Lines protocol over two dedicated unidirectional pipes.
Standard output and standard error are drained concurrently into bounded worker
logs, so unexpected library output cannot corrupt or deadlock the protocol.
Version 1 targets POSIX systems and passes the worker's inherited read and write
descriptor numbers through `METEWAND_PROTOCOL_READ_FD` and
`METEWAND_PROTOCOL_WRITE_FD`; a later version may specify Windows handle
transport. SDKs receive the protocol descriptors directly. Raw command workers
may explicitly request a stdin/stdout compatibility mode, in which case any
unexpected standard output is a protocol violation. Large arrays and dataset
artifacts are passed as paths to private artifact views, never embedded in JSON.

Protocol JSON uses the same numeric and Unicode subset as canonical parameters.
Messages are UTF-8 without a byte-order mark, contain no duplicate keys, end in
one newline, and are limited to 1 MiB in version 1. Current schemas reject
unknown fields. Extensions must use compatible capability negotiation or a
protocol version supported by both peers before sending new fields.
Oversized lines, invalid encoding, malformed JSON, early EOF, extra responses,
and mismatched request IDs are typed protocol failures. Logs have policy-defined
byte limits. Once a limit is reached, Metewand records truncation and continues
draining to a sink so the child cannot block on a full pipe.

Every request and response carries a string request ID. Version negotiation,
role, resolved worker identity, SDK version, and capabilities are established
before any work begins. Version 1 permits at most one orchestrator-initiated
request per worker, but IDs make timeouts, late responses, and diagnostics
unambiguous. A streaming `execute` request may contain the bounded checkpoint
event/decision subexchange defined below without permitting an unrelated
concurrent request. The orchestrator supplies a minimal allowlisted environment,
fixed working directory, locale, timezone, and umask; `HOME`, temporary
directories, and ecosystem cache directories point to private worker or attempt
storage so user configuration and caches are not inherited accidentally. Their
resolved templates and behavior-affecting values are recorded, while ephemeral
absolute paths are attempt provenance rather than identity inputs. Secrets are
passed only through explicitly declared redacted channels and are never
included verbatim in plans, logs, identities, or provenance.

The orchestrator begins a worker session with a handshake:

```json
{"id":"0","method":"hello","protocols":[1],"role":"implementation","worker_id":"mw1-worker-..."}
```

The worker responds with supported capabilities, such as:

```json
{"id":"0","ok":true,"protocol":1,"worker_id":"mw1-worker-...","sdk":{"name":"metewand-python","version":"0.1.0"},"capabilities":["one_shot","applicability","fresh_sequence","streaming_profile"]}
```

Direct protocol workers return `sdk: null`. The orchestrator rejects a worker
identity mismatch or a worker missing any selected declared capability;
additional capabilities are inert unless a later plan selects them.

Dataset materializers support:

```text
materialize(source_dir, dataset_parameters, dataset_seed, output_dir)
shutdown()
```

The materializer writes a canonical dataset manifest and files beneath
`output_dir`. The manifest records every file's relative path, media type, byte
hash, size, and, where relevant, logical shape, data type, byte order, and
sparse-layout convention. Metewand validates the manifest schema, verifies every
declared file, rejects undeclared entries, and runs any problem-contract dataset
validator before publication. JSON Schema alone is not treated as validation of
opaque binary data. A dataset definition without a materializer treats its
verified source tree as the dataset instance and therefore cannot accept
materialization parameters. After all files are closed, the materializer returns
the manifest path beneath its assigned output:

```json
{"dataset":{"manifest":"dataset-manifest.json"},"id":"1","ok":true}
```

Implementation workers support:

```text
check_applicability(dataset_dir, dataset_manifest, problem_contract_id,
                    problem_parameters, implementation_parameters)  # optional
prepare(dataset_dir, problem_contract_id, problem_parameters, implementation_parameters, implementation_seed)
execute(result_dir, optional_scientific_budget, optional_observation_control)
execute_stream(checkpoint_root, optional_scientific_budget)           # optional
execute_buffered(result_root, instrumentation, optional_scientific_budget)  # optional, planned
reset()
shutdown()
```

`check_applicability` is required only when the worker declares the
`applicability` capability. `execute` is used by `one_shot` and
`fresh_sequence`; the latter passes one canonical observation-control value from
the implementation's declared schema and schedule. `execute_stream` is required
for `streaming_profile`. The planned `execute_buffered` method requires the
`buffered_profile` capability and returns one completed buffer manifest after
the solve, without checkpoint events or continuation messages. That manifest
references the trace, canonical snapshot manifests, provisional stop reason,
clock and scope metadata, and total instrumented duration. All files are closed
and visible before the response; ordinary artifact, path, size, and hash checks
apply to every referenced file. Raw workers may implement this directly without
an SDK.

`execute` returns scalar metadata and a manifest of files under `result_dir`:

```json
{
  "id": "2",
  "ok": true,
  "implementation_time_ns": 18342011,
  "observation_control": {"iterations": 40},
  "result": {"manifest": "result.json"},
  "statistics": {"iterations": 37}
}
```

The Gate-1 one-shot request omits the optional scientific budget and observation
control, and its response omits `observation_control`. The `fresh_sequence`
subprotocol adds its selected control in the later profile slice; until then,
selecting that capability is rejected. `implementation_time_ns` is explicitly
nullable, and `statistics` is an object. Successful `prepare`, `reset`,
and `shutdown` calls return the common acknowledgment
`{"id":"…","ok":true}`. Successful `evaluate` calls instead return
`{"id":"…","ok":true,"verdict":"reached"}` after writing metrics. The required
`verdict` is `invalid`, `reached`, `not_reached`, or `not_required`: it applies the
problem-owned validity and one-shot completion rules. Transport success alone
does not imply scientific validity.

Evaluators may report the `problem_semantics` capability during the version-1
handshake. Metewand selects that capability when reported and includes the full
canonical `problem_semantics` object in `evaluate` requests, including reference
checks. Evaluators without it receive the original request shape. Python and R
SDK evaluators opt in with the `semantics` argument to `serve_evaluator`; their
callbacks receive the rules after problem parameters.

The canonical result schema belongs to the problem contract. For a lasso problem
it might require `coefficients` and `intercept`, regardless of how individual
libraries represent them. An `execute` response is sent only after canonical
result files have been closed and made visible. Their manifest uses the same
path, hash, size, media-type, and representation metadata as dataset manifests.
Result serialization is therefore inside every timing scope that includes
`execute`.

During `execute_stream`, the worker may emit a bounded `checkpoint` event
containing its sequence index, canonical observation control or work-unit
metadata, cumulative in-worker diagnostic time, statistics, and a completed
result manifest beneath its assigned checkpoint root. Metewand stops the active
execution timer only after the complete checkpoint is visible, validates the
result, invokes the independent evaluator, durably publishes the observation,
and replies with a `checkpoint_decision` of `continue` or `stop`. The decision
includes only orchestrator state and evaluator-owned target status; it does not
return trusted metric computation to the implementation. Sequence indices and
the policy's maximum observation count bound the subexchange, and a worker that
continues after `stop` or emits an undeclared checkpoint is a protocol failure.

Problem evaluator workers support:

```text
evaluate(dataset_dir, problem_contract_id, problem_parameters, result_dir, metrics_path)
```

They compute problem-defined quality and correctness metrics—for example,
objective values, sampling diagnostics, prediction error, numerical error, or
constraint violations. The orchestrator validates the completed metrics
document against the contract's metric schema and result-validity rules before
it retains an observation. It then applies the observation policy's completion
rule. Implementation-reported metric values may be stored as diagnostics but
are not authoritative.

Workers may be persistent according to the execution policy so interpreter
startup and package loading can be separated from problem execution.
`worker-reuse: false` provides a fresh process but is called process freshness,
not strong sandbox isolation. The reuse key includes role, definition, source
bundle, resolved environment, launch program, and policy. The orchestrator calls
`reset` between slots, clears slot-scoped writable directories, and requires
every `prepare` to establish clean state. Any cache allowed to persist across
slots is named explicitly by the policy and included in the reuse semantics. A
worker that times out, crashes, violates the protocol, or fails to reset is
discarded. If process-local warm-ups were requested, its replacement repeats
them before accepting a measurement.

A candidate advances through applicability resolution before any measured
attempt is reserved. A run attempt then advances through `reserved`,
`materializing`, `starting`, `preparing`, `executing`, repeated
`checkpointing`/`validating`/`evaluating` transitions when applicable, and
`finalizing` states. `accepted`, `censored`, and each typed failure are terminal
attempt outcomes. A one-shot or fresh-observation slot is satisfied only by an
accepted attempt. A streaming attempt may be censored by a time or observation
bound while retaining its durably finalized valid observations. State
transitions and timestamps are transactional, but an attempt or observation
becomes visible as complete only after its immutable directory has been durably
published.

The orchestrator validates every referenced path and output manifest, rejects
absolute paths, path traversal, special files, escaping links, and undeclared
files, and never trusts a worker-supplied path outside its assigned directories.
Each attempt receives a private dataset view and output directory. Strong
executors expose immutable datasets with read-only mounts. The local executor
prefers copy-on-write snapshots or copies. The trusted local executor uses
independently verified copies and checks their canonical objects as well as
their private views around every operation. If an executor can provide only
permission-based read-only views, Metewand verifies all dataset artifacts
immediately before and after each worker operation or checkpoint subexchange
and records immutability enforcement as best-effort. The trusted local executor
also reports best-effort enforcement because it cannot prevent undeclared
filesystem access. Any mutation invalidates the attempt with `dataset_mutation`
and discards its private view; if the canonical cache object itself
changed, Metewand quarantines it before another run can consume it.

Errors have stable machine-readable codes, a human-readable message, and
optional structured details. The orchestrator distinguishes at least invalid
configuration, unsupported or mismatched capability, materialization failure,
setup failure, timeout, resource-limit violation, dataset mutation, worker
crash, malformed protocol, invalid result, evaluator failure, persistence
failure, and internal error. A run attempt records one of these outcomes rather
than disappearing because it did not produce metrics. A timeout initiates an
executor-defined termination sequence—interrupt, bounded grace period, then
forced process-tree termination. An executor that cannot contain and terminate
the complete process tree reports that limitation before execution and cannot
satisfy a required process-control policy. Incomplete checkpoint outputs are
retained only as diagnostic artifacts and never selected as observations.
Previously finalized valid observations remain visible when a later checkpoint
or the enclosing streaming attempt fails.

At the worker boundary, any method may replace its success response with
`{"id":"…","ok":false,"error":{"code":"operation_failed","message":"…","details":{}}}`.
Version 1 admits `invalid_request`, `unsupported_operation`, `operation_failed`,
and `internal_error`; the orchestrator combines that worker-owned category with
the active phase when it constructs an attempt outcome. Phase deadlines cover
the complete request and response exchange, while the shutdown deadline also
covers clean process exit. A timeout or protocol violation discards the worker,
as does any failed `reset`.

The conformance kernel initially implements `one_shot` without a scientific
budget. The first general release additionally implements applicability,
canonical problem-defined scientific budgets, `fresh_sequence`,
`streaming_profile`, and `buffered_profile`; adaptive budget schedules and
checkpoint state restoration require later protocol capabilities.

The protocol and executor are not a security boundary for hostile benchmark
code. Benchmark repositories and workers are trusted inputs; isolation controls
protect reproducibility and limit accidental interference. Running untrusted
submissions requires a separately designed sandbox or an integration such as
BenchExec.

## 5. Reproducibility model

`metewand.lock` is a **lockfile of definitions, configurations, artifacts, and
native lockfiles**. It does not replace `renv.lock`, `uv.lock`, `flake.lock`,
Julia `Manifest.toml`, or OCI manifests; it records and binds their identities
into the benchmark.

It must contain at least:

- normalized manifest, execution-policy, and observation-policy hashes;
- hashes of dataset materializers, each problem's contract and evaluator,
  schemas, implementation adapters, and every declared source bundle;
- canonical dataset, problem, implementation, observation-control, and
  applicability-decision identities;
- dataset source hashes, materialization recipes, and output-tree hashes;
- hashes of native environment lockfiles;
- resolved runtime and package-manager requirements and fingerprints;
- for Nix environments, every evaluated local flake-input source identity, the
  flake lock, installable, output kind, target system, derivation, resolved
  program, realized outputs, and the digest of a recursive closure manifest;
- OCI image digests; tags alone are invalid in locked mode;
- resolved launch programs, allowlisted environment values, executor
  definitions, the exact Metewand build fingerprint, execution-semantics
  version, SDK versions, and wire-protocol versions;
- generator sources, parameters, seeds, environment identity, and expected
  output hash for generated datasets.

Artifact resolution may use the network. Actual benchmark runs default to no
network access.

Four environment-resolution classes are supported:

  | Mode             | Environment                     | Guarantee                                                                                                                                |
  | ---------------- | ------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
  | Development      | Existing local runtime          | Observed provenance only; convenient, but not reliably recreatable                                                                       |
  | Ecosystem-locked | `uv`, `renv`, later Julia `Pkg` | Declared dependencies and sources must match their locks; undeclared system and platform dependencies remain recorded limitations        |
  | Closure-locked   | Nix flake installable           | Evaluated source, derivation, realized store outputs, and dependency closure are identified; runtime kernel and hardware remain external |
  | Image-pinned     | OCI image by digest             | Immutable declared userspace image; host kernel, hardware, runtime, and explicitly mounted datasets remain external                      |

Environment resolution and execution isolation are independent dimensions. A
digest-pinned image does not by itself prove that networking, memory, CPU use,
or filesystem writes were constrained. Each executor advertises its controls,
and each run records every control as requested, enforced, best-effort, or
unsupported. With `enforcement: "required"`, planning fails if a declared
executor capability is known to be insufficient; runtime preflight or
enforcement failure still produces a typed failed attempt because host
permissions and facilities may change after planning. Best-effort execution
remains available but is never presented as equivalent to enforced execution.

Unless a contract states a narrower rule, resource, network, filesystem, and
process-tree controls begin before worker launch and cover its entire lifetime,
including untimed startup, evaluation workers, reset, and shutdown. Excluding a
phase from performance timing never excludes it from isolation or accounting
provenance.

An observation is valid only if all of the following hold:

1. every declared dependency resolves to its expected typed identity, and every
   resolved identity matches the selected lock state when locked mode was
   requested;
2. the worker completed the applicable result or checkpoint exchange and
   produced a schema-valid canonical result manifest;
3. the problem's evaluator accepted the result as valid under its contract;
4. every control marked required was observed as enforced throughout its
   applicable scope;
5. the finalized observation artifact and provenance record were durably
   published.

An attempt is `accepted` only when it completes its one-shot result, its selected
fresh observation, or its profile according to the policy's fixed or
target-based completion rule and durably publishes the enclosing attempt
record. A profile attempt that reaches a bound before its target is
`censored`, and an attempt that violates the protocol or result-validity rules
fails, but any earlier valid observations remain part of the result set with
their relationship to the terminal attempt outcome intact.

Acceptance is distinct from the environment reproducibility class and the
observed control class. An accepted development or best-effort attempt may be
useful, but exports label its limitations and compatibility readers do not
silently pool it with closure-locked, fully enforced attempts. Required
provenance fields may be selected by policy; an unavailable required field fails
preflight, while an unavailable optional field is represented explicitly.

The tool must not claim bitwise-identical results or identical performance
across machines. Every run records hardware and operating context, including CPU
model, architecture, selected cores, memory, kernel, container runtime, language
runtime, BLAS implementation, relevant thread variables, tool version,
repository commit, and dirty-tree hash. Where available, it also records CPU
governor, frequency/turbo state, NUMA placement, accelerator identity, and
background-load indicators. Missing provenance is represented explicitly rather
than silently omitted.

Timing fields are distinct:

```text
materialization_time   dataset generation or transformation
worker_startup_time    interpreter and package loading
prepare_time           the complete prepare request
implementation_time    monotonic in-worker timing around the implementation call
execute_request_time   executor-observed execute request and result serialization
timed_wall_time        orchestrator-observed selected timing scope
active_timed_wall_time cumulative timed execution excluding checkpoint evaluation
elapsed_wall_time      total elapsed profile-session time including evaluator pauses
instrumented_time_ns   worker-reported cumulative time at an internal check
instrumented_total_time_ns worker-reported complete instrumented scope duration
evaluation_time        independent result evaluation
cpu_time / peak_rss    executor-observed process-tree use where supported
```

The execution policy identifies a primary executor-observed timing measure
covering its complete timing scope. Measurements that cannot include the
complete worker process tree are marked partial and cannot satisfy a policy that
requires complete accounting. For a streaming profile, the authoritative curve
coordinate is cumulative `active_timed_wall_time`; `elapsed_wall_time` exposes
the orchestration and evaluation pauses rather than hiding them. A buffered
profile uses `instrumented_time_ns` as its explicitly instrumented curve
coordinate, retains both complete durations, and never describes an internal
timestamp as executor-observed. Warm-up and untimed preparation are recorded even
when excluded from the primary measure.

## 6. Environments, execution, and artifacts

Environment resolution and worker launch resolution are separate stages. An
environment backend produces a resolved software context, its fingerprint,
backend-required variables, and its declared launch capabilities:

```text
local     observe an existing host software context; provenance only
uv        create or sync a Python project with its lockfile
renv      restore an R project with its lockfile
nix       realize a selected flake output and its closure
oci       resolve a userspace image pinned by digest
```

The worker's runner then resolves a launch within that context. The result
contains the exact executable, argument prefix, worker entrypoint or program
arguments, allowlisted environment, and working directory. Built-in Python, R,
and Julia runners select the corresponding interpreter supplied or admitted by
the environment; the command runner selects an explicitly declared program.
Local launches retain the selected interpreter path when it is a symlink to an
external executable, preserving Python virtual-environment discovery. Executable
validation and content hashing still follow the symlink target.
Resolution fails if the environment cannot satisfy the runner. Environment
fingerprints and launch fingerprints are distinct: two workers may share one
resolved environment while using different interpreters or programs, and a
launch change does not pretend that the underlying environment changed.

Local source bindings remain path-independent in launch identities. An explicit
`{sources}/PATH` argument refers to a normalized path within the worker's
declared source artifact; interpreted entrypoints use the same binding.
Content-addressed external program selectors and environment path templates
are resolved to host paths only at launch. The exact host launch belongs to
provenance, while the stable launch representation binds program contents,
source artifacts, arguments, and behavior-affecting environment values.

For `uv` and `renv`, the native lockfile is necessary but not sufficient:
environment resolution also records the package-manager version, platform tags,
repository identities, hashes of every downloaded wheel, archive, or source
checkout, and a manifest of the realized environment. Launch resolution
separately records the exact interpreter. Local or editable dependencies are
declared source bundles. Locked execution uses the already realized environment
without network access, verifies its manifest before launch, and verifies the
selected executable before every use.

Julia `Pkg` and Conda may be added later. Conda is not a privileged dependency
and need not be supported initially, but its architectural contract is explicit:
one platform-specific locked prefix may satisfy Python, R, Julia, and command
launches without being split into artificial per-language environments. OCI
images and Nix package outputs obey the same rule.

### Nix environments

Nix is a first-class locked environment backend. A Nix environment names a flake
reference, an installable, a target system, and an `output-kind` of `package` or
`app`. The output kind selects the exact `packages.<system>` or `apps.<system>`
attribute namespace; Metewand does not reproduce the Nix CLI's fallback search
between namespaces. It may provide a shared environment for dataset
materializers, problem evaluators, implementations, or later analysis workers.
Standard flake outputs are the integration boundary, and Metewand does not
require a separate `devenv` backend.

For a package output, environment resolution realizes the selected derivation
outputs. Launch resolution selects an interpreter or an explicitly declared
command program as a normalized path relative to one designated output root. For
an app output, environment resolution realizes the app closure, and launch
resolution selects the app's store-resident `program`; the worker definition
owns only its arguments and protocol role. In every case, Metewand validates
that the resolved executable is a regular executable file in the realized
closure and locks its exact store path. It never guesses a binary name from a
derivation name.

The two resolution stages realize the closure and select the launch program
before any benchmark timing. Workers are launched directly from that program;
`nix run`, `nix develop`, evaluation, substitution, and build time are never
included in worker startup or problem-execution timing.

In locked mode, Metewand forbids impure evaluation and refuses any operation
that would create, update, or rewrite `flake.lock`. The environment fingerprint
records:

- the exact evaluated source hash of the root and every transitive local or
  `path:` flake input, plus the `flake.lock` hash;
- the selected installable, output kind, target system, and designated output;
- the Nix version and relevant evaluation configuration;
- the derivation path and realized output paths;
- NAR hashes for the outputs and their recursive runtime closure, stored as a
  content-addressed closure manifest.

The corresponding launch fingerprint records the exact store-resident program
and arguments selected from that environment. The program remains part of the
resolved run identity and `metewand.lock`, but it is not conflated with the
reusable environment fingerprint.

Recording realized NAR hashes matters because a store path identifies a build
recipe but does not, by itself, establish that independently rebuilt output
bytes are identical. Every local flake input is bound by the exact filtered
source imported by Nix rather than by `flake.lock` alone. Metewand supports an
explicit tested range of Nix versions and rejects versions outside it in locked
mode because flake and installable CLI behavior is not a stable integration API.
Metewand's environment cache creates GC roots for resolved outputs; only
explicit cache garbage collection removes them. Run records retain the complete
fingerprint after removal, and Metewand may recreate the environment by
rebuilding or substituting the locked installable, subject to source and cache
availability.

Nix remains an environment provider, not an executor. Its build sandbox does not
constrain the later benchmark process. CPU, memory, network, filesystem, and
process-tree controls remain the responsibility of the selected executor and are
reported with the same enforcement levels as every other environment backend.
Dataset artifacts and run outputs remain Metewand artifacts rather than Nix
store outputs.

Executor backends consume the launch specification:

```text
local process       Gate 2
OCI runtime         Gate 5 on Linux via Docker or Podman
SLURM/remote        later
```

On Linux, executors should support CPU affinity, thread-related environment
variables, process-tree memory and CPU accounting, a private writable run
directory, read-only datasets, and disabled networking. Setting thread
environment variables is not considered enforcement of a thread limit. The local
process executor may report controls as unsupported; OCI or a later BenchExec
backend may provide stronger enforcement. Do not build a generic sandbox or
scheduler in v1; preserve an executor interface so stronger isolation or an HPC
backend can be integrated later.

Performance runs are serial by default. `run-order: "randomized"` forms
comparison blocks by dataset instance, problem configuration,
implementation-replication index, and measurement index; it orders
implementation configurations within each block and the blocks themselves by
their deterministic scheduling priorities. This interleaves comparable
implementations without making seeds or identities depend on completion order. A
fresh-sequence profile adds observation rounds ordered by each implementation's
schedule index, allowing operating points from different implementations to be
interleaved without asserting that their control values are equivalent. A
streaming or buffered profile occupies one serial execution slot; its internal
observations are not interleaved with another implementation. A sequential order
remains available when explicitly requested. Concurrent execution is opt-in and
becomes part of the run identity and provenance because contention changes the
comparison conditions.

The execution policy's `max-concurrency` is a positive safe integer with default
`1`. It is an explicit member of the canonical policy even when omitted in the
manifest. Execution semantics version 2 divides each experiment's selected
schedule into consecutive batches of at most that many attempts. A batch
finishes, including durable publication, before the next batch begins. Batches
never cross experiment boundaries. Each position within a batch has its own
worker owner, which may retain an eligible implementation process across
batches. A smaller subsequent policy drops excess retained workers.

The orchestrator emits each batch's start events in schedule order before
dispatch and its terminal events in that same order after every dispatched
attempt finishes. It calls the event sink on the orchestrating thread. A sink
failure stops further dispatch while preserving already published attempts.
Every attempt, including a failed attempt, records the effective policy and
the configured bound, batch index, batch size, and lane index in provenance.
Concurrency bounds attempts, including their warm-ups and evaluators; worker
resource requests remain per worker. Dataset construction and launch resolution
still precede measured dispatch.

### Source acquisition and dataset materialization

Metewand owns reproducible construction, validation, storage, and protection of
benchmark inputs. Benchmark authors supply the scientific recipes for data
generation, fold selection, preprocessing, and format conversion through dataset
materializers. Optional providers such as Datamonger identify recorded datasets
and may supply verified decoding. Metewand coordinates these operations through
processes and explicit runtime interfaces.

The source-resolution boundary accepts three kinds of request:

- repository files selected by existing source-bundle declarations and pinned
  to the bundle identity established by repository checking;
- a URL and expected raw SHA-256 from the manifest; and
- an explicitly selected external provider with a canonical, immutable
  provider-specific selector.

`metewand-runtime::source_resolution` defines this boundary with
`SOURCE_RESOLUTION_VERSION = 1`. A successful resolution owns a private local
directory and returns its complete regular-file inventory, verified sizes and
SHA-256 digests, versioned local-tree digest, and a versioned provenance receipt.
The receipt retains the original selection and any acquisition evidence
separately from the content address. Workspace paths, retrieval locations, and
provider receipts do not enter that content address. The directory is temporary;
durable publication and cache leases remain separate operations.

Repository resolution copies declared inputs and checks both the original and
copied bundle against the expected identity. Repository staging stays outside
selected source directories and glob prefixes, and this initial boundary admits
regular files and directories only. URL and external requests require an
explicitly supplied acquisition backend. The backend honors the request,
validates provider-specific selector pins, exports independent files into the
assigned directory, and returns its
expected inventory. Metewand independently checks that inventory against the
output, rejecting duplicate, missing, extra, unsafe, or mismatched files. A URL
request produces exactly one file named `source`, preserving the downloaded
bytes and compression and matching the manifest's digest. Extraction follows
source verification.

Acquisition backends own their supported transports, operational deadlines, and
provider-specific verification. They use supported provider exports and preserve
provider ownership of caches. Decoded-value verification remains a distinct
provider claim; raw byte verification alone does not establish it. For example,
Datamonger's artifact-only CLI requires an additional explicit decoding step to
obtain that guarantee. External selectors and their resolution must never fall
back to floating dataset versions or ambient project configuration.

Acquisition occurs only during explicitly requested runtime resolution. `check`
and `plan` may inspect declarations and hash repository files, but never download
sources, invoke acquisition backends, or run materializers. Materialization then
consumes resolved sources and produces the canonical dataset instance. All
selected implementations consume private views of that instance under the
problem contract's preparation and timing rules.

Shared materialization defines the input to the comparison. It may generate
data, select folds, or apply contract-defined scientific preprocessing, but it
does not receive implementation parameters or seeds and must not perform one
implementation's preparation on behalf of every candidate. Implementation-owned
conversion, such as packing the canonical values into a library's native array
layout, belongs to `prepare` or `execute`. It must preserve the problem instance
and leave the dataset view unchanged; workers keep converted data in memory or
their private working directories. A conversion to a different dataset schema
requires an explicit shared dataset materializer. A change to the requested
scientific computation belongs to the dataset or problem configuration.

The contract's validated, hashed family semantics specify which implementation
preparation may occur outside the timed scope. Such permission does not move
work outside the selected policy's timer: `prepare_and_execute` includes all
preparation, while `execute_only` requires precise rules for untimed preparation
and produces an operation-scope comparison. These rules and the
[materialization and preparation conformance cases](docs/reference/dataset-materialization.md#materialization-and-implementation-preparation)
complete existing version-1 semantics without adding a manifest field, wire
message or identity transcript. All three envelopes are implemented in the
local runtime; its [timing policy](docs/reference/fixed-dataset-run.md) distinguishes the
shared scientific deadline from operational phase limits. Diagnostic measurement
payload version 2 records the corresponding CPU accounting scopes.

The generic source-resolution boundary introduces no new manifest syntax,
serialized public schema, identity transcript, wire message, or execution timing
rule. Existing compatibility constants remain unchanged. Changes to provider
syntax or serialized source receipts follow the same compatibility policy as
other external boundaries. An explicitly selected HTTP(S) backend now supports
bounded pinned downloads.
A separate extraction API admits tar and tar.gz regular files and directories
with mandatory entry-count and expanded-size limits. It rejects all links,
special entries, traversal, duplicate paths, and tar metadata extensions.
Extraction rechecks the source before use and leaves archive permissions and
ownership unapplied. The optional Datamonger adapter invokes the public Python
client in an isolated, bounded process with an explicit strong registry selector
and dataset version. It exports independently verified copies of raw artifacts,
preserves provider cache ownership, and separately records artifact-only or
decoded-value verification. A decoded request must verify the registered
representation's shapes and canonical digest or fail without a downgrade.
Its selector and standalone command have initial version-1 public schemas under
`DATAMONGER_ADAPTER_VERSION = 1`; existing compatibility constants and manifest
representations remain unchanged. The [runnable example](examples/datamonger)
constructs a standard dataset and compares implementations using private views
of the same instance after source resolution, with no further provider access.
See [Datamonger sources](docs/reference/datamonger.md) for limits, evidence, and offline
conformance tests. The current local `run` path uses the repository branch of
this boundary for fixed data and an explicit HTTP(S) backend for
generated datasets.

Artifacts live in a content-addressed cache. Downloaded files are verified
before extraction. Local trees are hashed from sorted normalized paths, entry
types, file bytes, symlink targets, and semantically relevant executable bits;
timestamps and ownership are excluded. Archives are extracted with traversal,
link-target, size, and entry-count checks; device nodes, sockets, FIFOs,
escaping hard links, and unsupported entry types are rejected.

The runtime now executes generators and transformations through
`dataset_materialization::materialize_dataset`, with canonical manifests,
planned dataset seeds, complete inventory checks, and offline output-schema
validation. Runtime callers may select a contract-owned binary checker through
the separately versioned dataset-validation command interface. The checker
receives a canonical request and accepts only by exiting zero within its
operational deadline. Source or output mutations reject materialization.
The current manifest does not declare that optional checker hook.

External acquisition evidence used for materialization must record the resolved
dataset version, decoding recipe, and decoded-value verification status.
Provider claims remain distinct from Metewand's independent raw-byte checks.
The version-1 `dataset-derivation` identity binds these semantics and the verified
source trees to the resolved materializer, canonical parameters, and dataset
seed. Construction receipts record output hashes separately. See
[Dataset materialization](docs/reference/dataset-materialization.md) for the runtime
interfaces, standalone validation command, and compatibility review. The local
`run` command materializes each selected dataset configuration once per run or
reuses its verified construction from an explicit `--cache-dir`; `check` and
`plan` remain side-effect-free.

A dataset derivation identity covers the dataset definition, verified sources,
materializer source and environment, canonical parameters, and dataset seed. The
materialized output receives a separate content hash. The cache records the
mapping between them; in locked mode, regeneration must reproduce the expected
output hash or fail.

Artifacts and run outputs are written to uniquely leased staging directories on
the same filesystem as their destination. Finalization validates the complete
manifest, computes content hashes, synchronizes every file and directory,
publishes with an atomic no-replace rename, and synchronizes the parent
directory before committing the terminal database transaction. If another writer
won the same content address, Metewand verifies the existing object byte for
byte and discards its duplicate staging object. Normal operations expose a
finalized object only when its manifest contains a completion marker written
last and the database references the same content identity. Recovery may
validate and import a completed orphan left by a crash between publication and
the terminal transaction.

The initial filesystem store implements this artifact boundary through
`artifact_store::ArtifactStore` with `ARTIFACT_STORE_VERSION = 1`. It publishes
content trees and construction mappings separately, writes completion metadata
outside the addressed tree, and gives each local attempt an independent,
verified copy. `import_source` copies verified provider exports while preserving
provider ownership. Kernel locks protect unique staging, reader, and view
leases for their owners' lifetimes; explicit recovery quarantines abandoned
lease staging only after acquiring its released lock. The CLI uses a temporary
store unless `--cache-dir` selects persistent construction reuse. The new object,
mapping, and lease schemas join the initial version-1 public catalog without
changing identity or tree-hash transcripts. Readers, view transfers, and
publishers serialize verification and canonical-object quarantine with a kernel
lock. Quarantine atomically moves the exact changed object out of the live
address space and synchronizes both parents before releasing the lock. Retained
derivation mappings constrain any reconstruction to the original output hash.
These controls complete the unreleased version-1 immutability contract without
changing its compatibility constants. The separate run-store publisher connects
leased publication and terminal database references as described in [Durable run-store
schema](docs/reference/run-store-schema.md#transactional-publication). Its explicit
[recovery API](docs/reference/run-store-schema.md#explicit-recovery) resolves expired
attempts, claimed orphans, and abandoned stages. The CLI uses this path for
explicit retries and resume. See
[Artifact storage and private dataset views](docs/reference/artifact-store.md).

Later reads verify hashes whenever data crosses from the cache into a worker
view, before evaluation, and during export or explicit cache verification.
“Immutable” therefore means that mutation is prevented when practical and
detected before an artifact can contribute to an accepted attempt. A corrupted
object is quarantined transactionally. Cache garbage collection takes a database
snapshot, respects active leases and GC roots, and deletes only exact
unreferenced content identities; an index or failed cleanup can never authorize
deletion by a broad path.

The local cache implements this boundary with a WAL/FULL SQLite catalog of
verified object hashes, named roots, and exact collection intents. Persistent
runs pin their datasets before registering run-store slots. `cache verify`
checks complete tree identities, `cache roots` lists retention edges, and
`cache gc` takes a fresh transactional snapshot while holding the cache lock
through lease checks and deletion. Verification and `--dry-run` leave cache
files unchanged. Interrupted collection resumes only its recorded exact
identity. See [Cache verification, roots, and garbage collection](docs/reference/cache-maintenance.md).

Finalized attempt outputs are immutable directories containing:

```text
attempt.json          slot, retry, outcome, observation references, and provenance
logs/                 role- and attempt-scoped worker output
manifest.json         completion marker plus hashes and sizes of all other files
```

Each one-shot result, streaming checkpoint, or returned buffered snapshot is
independently finalized before the attempt record may reference it:

```text
observation.json      slot, producing attempt, control, timing, and validity
metrics.json          evaluator output
result/               canonical implementation result files
manifest.json         completion marker plus hashes and sizes of all other files
```

Publishing observations separately allows a later timeout or worker crash to
leave earlier valid convergence points durable without exposing an incomplete
attempt directory as successful.

A top-level JSON Lines or Parquet index is a rebuildable projection of the
metadata database and finalized attempt and observation directories, never
authoritative state. It includes applicable and not-applicable candidates,
valid observations, and accepted, invalid, censored, superseded, and failed
attempts. Metrics are nullable and carry schema version, units, and comparison
direction where applicable. The schema preserves applicability evidence,
observation controls and target status, slot and retry identities, pairing keys,
component seeds, warm-up status, timeout limits, run order, machine/environment
blocks, observed controls, guarantee classes, and the execution policy's
authoritative timing measure. Profile records additionally preserve delivery
mode, measurement basis, instrumented scope and clock metadata, trace references,
provisional metrics and stop reasons, and evaluator-confirmed target status.
Measurement evidence remains separate from environment and control guarantees.
Metewand never silently averages repetitions or drops failed attempts.

The local foundation implements this projection with version-1 `export-record`
and `export-row` schemas. `metewand export --output-dir PATH --rebuild-index`
explicitly rebuilds `index.jsonl`; `--destination PATH --format jsonl|csv`
creates a new table directory with an adjacent lineage receipt. Both read a
committed snapshot and verify registered artifacts independently of the index.
See [Result indexes and tidy exports](docs/reference/exports.md).

Every export has an adjacent or enveloping export manifest containing the
canonical selector, the ordered source candidate, attempt, and observation
identities, the result and metric schema identities, compatibility classes, the
export schema and tool versions, and a content hash of each emitted file. This
manifest gives a committed CSV or Parquet figure table an exact lineage without
making the downstream statistical transformation part of Metewand.

After the local MVP, `metewand snapshot create` will package a selected result
set as a portable, content-addressed directory or archive. A snapshot contains its export manifest,
the selected applicability, attempt, and observation records, referenced
canonical results and metrics, required schemas and contracts, selected lock
state when available, and the resolved dependency and provenance records needed
to interpret them. It contains no cache or workspace paths. `snapshot verify`
validates the complete Merkle graph and may run without a local Metewand store;
`snapshot import` verifies first and adds objects with atomic no-replace
publication. Snapshotting does not upgrade the recorded environment or control
guarantee of any attempt.

Metewand owns this analysis-ready schema and its provenance, but publication
analysis and graphics are not part of the execution core. The R, Python, and
Julia packages provide typed result and snapshot readers and uncontroversial
operations such as filtering warm-ups, pairing compatible attempts, and
validating result-set compatibility. Statistical estimators and publication
plots remain downstream code owned by the benchmark author.

A later analysis layer may produce immutable, provenance-tracked artifacts:

```text
AnalysisArtifact = result set + analysis definition + parameters + environment
```

Analysis artifacts do not alter the identity or acceptance status of their
source attempts. This allows statistical choices to evolve without rerunning
implementations while keeping the analysis reproducible.

Metewand may provide a read-only interactive diagnostic viewer over the same
public result schema. Its purpose is to inspect run coverage, failures,
parameter slices, raw metric distributions, timing, and provenance. Views are
deliberately fixed and generic; the viewer does not provide a plotting grammar,
themes, figure composition, annotations, statistical-test selection, or
publication-format guarantees. It may export the selected underlying data, but
publication figures are produced in R, Python, Julia, or another downstream
tool.

## 7. Initial SDK APIs

The R and Python packages should be intentionally small and usable without
knowledge of the orchestrator internals. Add callbacks as adopter workflows
need them. The Julia SDK follows the local MVP; raw executables, including
Rust workers, can use the protocol directly.

One-shot convenience callbacks let an implementation return
scientific result data and an evaluator return metric data with an explicit
validity and completion verdict. Rust helpers live in
`metewand_runtime::worker_sdk`; Python provides `OneShotImplementation` and
`Evaluation`; R accepts execute output with optional `result_data` conversion
and provides `evaluation`. Existing lifecycle callbacks and custom writers
remain available. Helpers handle handshakes, protocol transport,
errors, standard artifact envelopes, and artifact writing. Adapters retain
dataset preparation, solving, scientific result conversion, and metric
formulas. Give evaluators the resolved scientific rules so manifest settings
need not be repeated in their source code. Preserve the declared timing scope,
including result serialization where required. Native Rust workers must not
require a Python host.

Keep the lower-level lifecycle hooks and custom artifact writers available for
benchmarks that need them. The following sketches show that lifecycle and the
planned profile hooks; convenience helpers do not require every simple adapter
to implement all of them.

R sketch:

```r
metewand::serve_implementation(
  applicable = function(
    dataset_instance, problem_contract, problem_parameters, implementation_parameters
  ) { ... },
  prepare = function(
    dataset_instance, problem_contract, problem_parameters, implementation_parameters, seed
  ) { ... },
  execute = function(state, budget, observation_control) { ... },
  execute_stream = function(state, budget, checkpoint) { ... },
  execute_buffered = function(state, budget, trace) { ... },
  result = function(state, directory) { ... }
)
```

Python sketch:

```python
from metewand import Implementation, serve_implementation

class SklearnImplementation(Implementation):
    def applicable(
        self, dataset_instance, problem_contract, problem_parameters,
        implementation_parameters
    ): ...
    def prepare(
        self, dataset_instance, problem_contract, problem_parameters,
        implementation_parameters, seed
    ): ...
    def execute(self, budget, observation_control): ...
    def execute_stream(self, budget, checkpoint): ...
    def execute_buffered(self, budget, trace): ...
    def write_result(self, directory): ...

serve_implementation(SklearnImplementation())
```

The later Julia API will provide the same callbacks using ordinary Julia
functions and structs and share the canonicalization, seed, protocol, and error
vectors used by R and Python. Its implementation and conformance suite do not
block the local MVP.

For the planned buffered API, `trace.record` accepts measurements supplied by
existing instrumentation; `trace.observe(progress, measure, snapshot)` is the
local observer passed into a cooperative solver. Both enforce the selected
schema, schedule, clock boundary, and collection bounds. `trace.finish` captures
any required terminal check and snapshot before result serialization. A solver
integration looks like this:

```python
def execute_buffered(self, budget, trace):
    state = solve(self.inputs, budget=budget, observe=trace.observe)
    trace.finish(state.progress, state.measure, state.snapshot)
```

The solver supplies measurement and snapshot functions for its current iterate,
and checks each callback's `continue` or `stop(reason)` return before advancing.
The SDK dispatches local measurements and applies the declared provisional
stopping rule; scientific metric formulas remain benchmark-owned code. Native
solvers may implement the observer without calling Python, R, or Julia per
iteration and submit the equivalent buffer through the worker adapter. The
blocking streaming `checkpoint` callback and the local buffered `observe`
callback are distinct APIs with different timing and durability guarantees.

An SDK may report monotonic `implementation_time` around only the user execute
callback. It writes each canonical result before replying to `execute` or
invoking the streaming `checkpoint` callback, so the orchestrator's active
timing envelope includes serialization. The SDKs also provide analogous, small
helpers for dataset materializers and problem evaluators. They must not
provision environments, acquire source artifacts, choose observation schedules,
define parameter semantics, or calculate trusted metrics independently of
user-supplied evaluator code. Buffered helpers may apply the manifest's declared
schedule and provisional local stopping rule; only independent evaluation
establishes accepted target attainment.

## 8. MVP and acceptance criteria

The immediate goal is useful local benchmarks in Basin and clustlasso-cd.
The completed foundation remains available while the remaining modes are added.
[TODO.md](TODO.md) gives the active implementation order.

### Gate 1: conformance kernel

The implemented kernel parses and validates manifests, contracts, canonical
values, source bundles, identities, and deterministic plans. The bounded worker
protocol runs a fixed dataset through an implementation and independent
evaluator and publishes a self-describing observation and attempt.

### Gate 2: reliable local foundation

The implemented local runner adds verified source acquisition and generated
datasets, private artifact views, durable records and recovery, explicit retries,
timing policies, worker reuse, scheduling, inspection, and JSONL and CSV exports.
Python and R SDKs and runnable examples exercise the process boundary.
Existing foundation regressions continue to run.

### Gate 3: comparative local MVP

Fresh-sequence and buffered profiles are required; streaming profiles follow
the MVP. The scope supports practical use in the two adopter projects:

1. Run small Basin and clustlasso-cd comparisons through the existing one-shot
   runner. Simplify authoring with flat inline problem definitions, compact
   parameter, result, and metric declarations, default selections, mechanical
   defaults, and an optional family semantics schema. Provide small adapters
   that return scientific data while helpers handle protocol and artifact
   bookkeeping. Keep scientific choices, independent evaluators, and reference
   cases explicit.
2. Add configuration-dependent applicability with checked-input evidence and
   visible exclusions that do not become failed attempts.
3. Add buffered instrumented profiles first, reusing native local traces with
   selected immutable snapshots, direct recording, and local observers.
   Independently evaluate saved results and attribute local timing and stopping
   claims to their instrumentation.
4. Add fresh sequences of implementation-specific controls from clean
   algorithmic state, with fixed-count or evaluator-owned target completion.
5. Qualify small real comparisons through checking, planning, selected execution,
   interruption, resume, inspection, and usable exports. Add the SDK
   callbacks and scientific budgets those comparisons actually need.

| Adopter | Required use |
| --- | --- |
| Basin | Run a Rust optimization method and an existing competitor on shared cases. Retain final parameters, independently checked accuracy or feasibility, repeated solve timings, and separately labeled instrumented curves. Reuse the existing competitor harness and local observation points. |
| clustlasso-cd | Refresh the existing Python and R package integration for the current CLI, then run internal native trajectories with selected coefficient snapshots. Reuse fixed inputs and independent evaluation, keep package and internal tracks separate, and preserve each track's declared scientific criteria. |

The native trace path must not require a Python or R callback at every check.
Existing metric-only traces can remain diagnostics; independently accepted
profile observations require saved results that the evaluator can inspect.
Several instrumented thresholds may share one unchanged trajectory under the
timing and snapshot rules in
[Instrumented profiles and local observers](#instrumented-profiles-and-local-observers).

Local use can begin with the one-shot integrations and grow with each mode.
The MVP is complete when the two adopter workflows are practical and both MVP profile
modes satisfy their behavioral tests. It provides development-environment and
observed-control guarantees. It does not depend on streaming profiles, the Julia SDK, complete
ports of the earlier intercept and SLOPE studies, portable result archives, or
environment provisioning.

#### MVP verification

Keep the existing foundation tests and add focused evidence for new behavior:

- canonicalization, identities, and seed vectors agree across the implemented
  Rust, R, and Python boundaries; unrelated definitions do not invalidate
  existing runs, and actual dependency changes invalidate their dependents;
- configuration expansion preserves namespaces, coupled cases, repetitions,
  seed sharing, and deterministic scheduling;
- compact and external declarations validate offline, reject invalid input at
  the author's declaration or supplied value, and record normalized schemas and
  effective defaults; equivalent declarations retain component identities;
- declaration selections, experiment overrides, and named cases preserve the
  valid parameter domains and existing expansion rules; evaluator callbacks
  consume declared scientific settings without repeating constants;
- independent evaluators accept reference solutions and reject malformed,
  numerically wrong, and semantically invalid results;
- applicability retains exact inputs and reasons, protects dataset views, and
  distinguishes exclusions from execution failures;
- fresh sequences reset algorithmic state, preserve process-reuse and warm-up
  rules, and stop from common evaluator metrics;
- buffered profiles preserve snapshots, bound collection, include measurement
  and callback costs, distinguish provisional from independent metrics, and
  never accept observations from an unreturned buffer or splice retries;
- timing probes verify the selected scope, failure evidence, and scientific
  deadlines; instrumented curves remain distinct from executor-observed curves;
- interruption and recovery do not manufacture success or duplicate selected
  observations, including profile publication boundaries;
- status and exports retain valid observations, exclusions, censored attempts,
  retries, and failures with their original timing and guarantee classes;
- small Basin and clustlasso-cd runs exercise the public commands and feed a
  usable downstream summary.

Use byte-level vectors where canonical bytes are the contract. Match other
tests to the behavior at risk. Extend supported SDK callbacks as needed without
requiring every language to implement every mode or a separate public schema for
each internal helper. Exhaustive durability testing follows the existing
change-scoped policy in [AGENTS.md](AGENTS.md).

### Gate 4: locked environments

After the local MVP, add `metewand.lock` and the Nix package and app backend,
followed by `uv` and `renv` as adopter demand warrants. Keep environment
fingerprints separate from exact worker launch identities, and keep Nix optional.
Qualify locked execution with transitive source, lock, realized-environment,
closure, and executable mismatch tests. Development runs retain their weaker
recorded guarantees.

### Gate 5: isolated archival execution

Add digest-pinned OCI images, enforceable resource and network controls,
read-only mounts, complete process-tree accounting, and archival machine
provenance. Docker and Podman on Linux must pass control-loss and accounting
tests before stronger publication-archive claims are made. These claims remain
separate from local MVP acceptance.

The Julia SDK and full intercept-strategy and SLOPE workflow ports follow the
local MVP, along with portable result archive creation, verification, and import.
Later extensions include richer scientific-budget schedules, checkpoint-state
restoration, and remote execution. Result snapshots within profiles remain part
of the MVP; portable archives of selected runs are a separate feature.

### Explicit non-goals for v1

- inventing a universal package manager or lockfile;
- embedding language runtimes or transferring native language objects;
- replacing general workflow engines, BenchExec, or SLURM;
- distributed execution, hosted benchmark registries, or a remote multi-user
  service;
- embedded language models or provider-specific agent orchestration;
- publication-ready plotting, figure composition, or reporting;
- bitwise-identical performance across hardware;
- automatic adapters for arbitrary computational APIs;
- native Windows worker-handle transport in version 1.

The central invariant is:

> An attempt is accepted only when every declared dependency resolves to its
> expected typed identity, every selected observation satisfies the problem's
> validity contract, the observation policy's completion rule is met, all
> required controls were enforced over the selected timing scope, and the
> complete attempt and its observations were durably published. Censored and
> failed attempts retain earlier valid observations without being reported as
> accepted. Environment and control guarantee classes remain explicit and
> independent of the language in which any worker is implemented.
