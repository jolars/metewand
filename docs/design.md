# Design

Metewand compares computational methods as implemented in software. A benchmark
can combine Python packages, R packages, and native executables while using the
same datasets, scientific criteria, and execution policies.

This document describes the general design. The [contributor
guide](https://github.com/jolars/metewand/blob/main/CONTRIBUTING.md) covers
implementation contracts and verification; the
[TODO list](https://github.com/jolars/metewand/blob/main/TODO.md) groups remaining
work by area.

## Scope

A benchmark performs a bounded computation and produces a result that another
worker can evaluate. Optimization, estimation, sampling, simulation, prediction,
and numerical integration fit this model. A long-running service without a
bounded result artifact does not.

The central comparison combines:

```text
dataset + problem + implementation + environment + execution and observation policies
```

Definitions describe families of inputs or methods. Parameters select concrete
configurations. An experiment selects definitions and policies, expands their
parameter choices, and requests repetitions of the resulting comparisons.

## Principles

- **Workers are processes.** The orchestrator does not embed a language runtime.
  Every language uses the same versioned process boundary.
- **The problem owns evaluation.** Implementations return scientific results; a
  separate evaluator computes trusted metrics and decides validity and
  completion.
- **Decisions are explicit.** Scientific parameters, algorithm settings,
  execution controls, and observation controls have separate owners.
- **Evidence determines guarantees.** A local development run, a locked
  environment, and an isolated execution make different claims.
- **Plans and identities are deterministic.** Equivalent normalized inputs
  produce the same identities, expansion, and scheduling decisions.
- **History survives failure.** Attempts, observations, retries, and exclusions
  remain inspectable. A retry does not overwrite earlier evidence.
- **Automation uses the same records as people.** Commands are noninteractive,
  and human output is rendered from typed machine records.

## Authoring

A small benchmark needs a YAML manifest and ordinary Python or R functions. The
manifest declares data generation, result and metric shapes, methods, and
experiments. Functions generate data, solve the problem, and evaluate results.
The language package supplies stable callback registration and scientific result
values. The CLI supplies its worker adapters and owns transport, standard artifact
envelopes, canonicalization, and file inventories. Binding API version 1 permits
compatible CLI development without corresponding language-package releases.

Authors can add parameter grids, scientific rules, analytic reference cases,
repetitions, and named policies as the comparison grows. Separate contracts and
JSON Schemas support reuse. Lifecycle callbacks and custom artifact writers
support separate preparation, binary data, native workers, and profiles.

Static checking and planning do not import benchmark code, launch workers, or
create run output. Worker checking explicitly authorizes imports and reference
evaluation. Running explicitly authorizes execution and publication beneath
`.metewand/` beside the manifest, or a directory selected with `--output-dir`.
Authoring and maintenance commands require explicit mutation requests.

Running again with the same output directory continues its durable history.
Matching accepted measurements are reused, and interrupted attempts restart from
clean state after recovery. Recorded execution or scientific failures require
an explicit retry. A new output directory starts an independent run.
Status and explanation commands read the default run alongside the current
manifest when it exists, without creating or recovering it.

## Scientific comparison

### Datasets and parameters

A dataset definition acquires, generates, selects, or transforms data. Each
materialized instance has a derivation identity and a separate output-content
hash. Dependent methods receive verified private views of the same data.
Dataset-free problems use a versioned empty dataset.

Dataset parameters control the input. Problem parameters control the requested
computation. Implementation parameters control how a method computes its answer.
Budgets and execution settings remain policies. Metewand preserves the author's
classification rather than guessing from parameter names.

Named download caches are explicit local runtime bindings. Their host paths
remain outside scientific parameters, seeds, launches, and derivations, while
provenance records effective bindings. Providers verify pinned content before
returning scientific data. Metewand's artifact cache separately stores verified
materialized outputs. An explicitly initialized shared artifact cache publishes
immutable, hash-linked retention snapshots instead of a shared SQLite catalog.
Portable run owners and live kernel leases protect objects during collection;
worker views stay in local scratch. Scientific overrides retain their normal
identity rules.

A problem may prepare shared inputs from the verified dataset and its resolved
parameters. Preparation runs once per dataset and problem configuration before
measured work. Its cached artifact has a separate content hash, and every method
and evaluator receives a verified private view of the same inputs.

### Validity and completion

A problem contract defines accepted datasets, result and metric shapes,
scientific rules, and allowed measurement scopes. Its evaluator independently
computes metrics and returns one of four verdicts:

  | Verdict        | Meaning                                                    |
  | -------------- | ---------------------------------------------------------- |
  | `invalid`      | The result violates the problem's scientific requirements. |
  | `reached`      | The result satisfies the declared completion target.       |
  | `not_reached`  | The result is valid but misses the target.                 |
  | `not_required` | No completion target applies.                              |

Schema validation establishes the shape of a result, not its scientific
correctness. Metrics do not imply convergence. Declared scientific rules require
an explicit evaluator verdict. Reference cases check the evaluator against
independently justified results and expected metrics.

Applicability is separate from failure. An implementation that cannot support a
configuration can report an exclusion with structured reasons before any
measured attempt. Exclusions must remain visible.

### Timing

The execution policy selects an envelope that the problem contract permits:

  | Scope                 | Included work                                                                     |
  | --------------------- | --------------------------------------------------------------------------------- |
  | `cold_end_to_end`     | Worker startup, preparation, execution, and result serialization.                 |
  | `prepare_and_execute` | Preparation, execution, and result serialization.                                 |
  | `execute_only`        | Execution and result serialization, with contract-authorized untimed preparation. |

Dataset construction and independent evaluation remain outside these scopes. The
default is `prepare_and_execute` with executor-observed wall time.

For one-shot and fresh-sequence solver comparisons, an execution policy can
select `primary-time: implementation_time` with `timing-scope: execute_only`.
This selects the worker's elapsed time around its execute callback, excluding
Metewand's result conversion, serialization, file writing, and transport. The
problem must authorize execute-only timing and describe allowed untimed
preparation. Method-specific setup belongs inside the callback when the
comparison charges that work. Conversion or serialization performed by the
callback itself remains included.

Both callback time and the executor's outer wall time remain recorded. Reports
identify the selected clock, and fresh points label worker-reported timing
separately from executor-observed timing. A missing selected callback measurement
fails the attempt without falling back to another clock. Buffered and streaming
profiles retain their separately declared trajectory clocks.

Scientific wall-time limits apply to the executor-observed envelope even when
callback time is primary. Fresh-sequence cumulative limits also charge that
envelope. Operational phase deadlines bound worker exchanges separately.
Resource measurements distinguish
complete, partial, and unavailable evidence. Missing evidence never becomes a
measured zero or a stronger guarantee.

### Instrumented profiles and local observers

One-shot execution produces one final result per measurement. Profiles retain
multiple observations under an explicit observation policy.

Fresh sequences run a finite schedule of controls from clean state,
independently evaluate each result, and stop when their common target or limits
require it. Each point has executor-observed time. Cumulative measured work and
stopping decisions belong to its repetition and survive resume.

Buffered profiles let an instrumented implementation collect a bounded local
trace and selected result snapshots during one trajectory. The evaluator checks
snapshots independently. Local provisional metrics help decide what to capture
but do not replace trusted metrics. A named instrumented scope defines the
clock's meaning, and exports keep that coordinate separate from wall time.

Streaming profiles pause one trajectory at bounded checkpoints. The runner
independently evaluates and durably publishes each snapshot before replying
`continue` or `stop`. Executor-observed active time includes checkpoint
serialization and excludes validation, evaluation, and publication pauses.
Elapsed wall time and evaluation time remain separate evidence.

Retries never splice trajectories together. Partial evidence can remain
inspectable without satisfying the observation policy. A streaming retry starts
from clean state; it does not restore implementation state from a snapshot.

## Planning and identity

Planning normalizes declarations, resolves defaults, validates contracts,
expands grids and named cases, and assigns stable logical identities. Logical
identity describes the requested comparison without depending on where a
checkout or temporary directory lives.

Resolution binds actual input content, worker launches, and environment
evidence. Resolved identities describe those concrete inputs. Changes to
sources, data, scientific rules, or execution policies invalidate their
dependents. Incidental timestamps and allocated paths do not define scientific
identity.

Dataset, implementation, and scheduling seeds use separate deterministic
derivations. Comparable implementations receive paired implementation seeds.
Benchmark code remains responsible for seeding its random generators.

The scheduler visits experiments and dataset configurations in expansion order.
Randomized scheduling shuffles implementation configurations within each
comparison block while preserving problem and repetition order. Concurrent
batches stay within one dataset configuration. Measurement repetitions,
implementation repetitions, and retries represent different things and retain
separate indices.

## Architecture

Metewand is one binary package with internal modules that separate domain logic
from execution:

  | Module         | Responsibility                                                                       |
  | -------------- | ------------------------------------------------------------------------------------ |
  | `core`         | Manifests, contracts, canonical values, identities, plans, scheduling, and run records. |
  | `protocol`     | Versioned wire types and bounded framing.                                             |
  | `runtime`      | Sources, artifacts, environments, worker execution, provenance, and durable storage.  |
  | `cli`          | Commands, diagnostics, and human and machine rendering.                               |

Core and protocol are independent foundations. Runtime may use both; the CLI
composes all three. Filesystem and execution concerns stay out of core and
protocol. Domain types stay out of the CLI. These modules are implementation
details; the package does not expose a Rust library API.

Workers negotiate roles and capabilities before receiving work. Implementations
prepare, execute, and optionally reset for reuse. Dataset materializers and
evaluators have separate roles. Protocol pipes are separate from worker logs.
The orchestrator validates completed artifacts before accepting their evidence.

## Reproducibility model

Reproduction requires more than a random seed. Recorded evidence identifies
scientific definitions, resolved parameters, sources, datasets, software,
execution controls, measurement scopes, and outcomes.

Canonical JSON, typed hashes, and explicit compatibility versions make these
records comparable across languages. Public boundaries are versioned from their
introduction. Package releases are separate from format and execution versions.
Supported historical records keep their original meanings.

HPC execution needs a durable run home on shared project storage, disposable
worker scratch, and host-local query indexes. Durable records must preserve
ownership, retry authorization, accepted observations, and scientific budget
decisions as well as result artifacts. Independent jobs should own disjoint
shards rather than update a shared SQLite WAL database. The opt-in shared run
backend publishes an immutable transaction journal and artifacts there, rebuilding
SQLite in host-local scratch on every invocation.
Each shared run has one coordinator. Disjoint distributed shards and qualification
on specific HPC filesystems remain future work.

Portable result archives provide the first recovery boundary. An archive retains
terminal attempts and pending work independently of SQLite. Restoring it rebuilds
a resumable local run with the original identities, selections, and profile
accounting. Shared runs additionally publish ownership, retry authorization,
observations,
acceptance, and budget decisions at each coordination transaction. Published
records commit before success or a streaming checkpoint reply. Recovery rebuilds
local coordination from that history, retaining interrupted ownership leases
until expiry and preserving conservative charges for unknown execution time.

The current local backend runs trusted code in development environments. It
records observed controls and limitations. Private directories and integrity
checks do not establish a complete dependency closure, network isolation, or
process-tree containment. Benchmark authors must also assess hardware and
scientific comparability; matching identities do not guarantee identical timings
across machines.

### Nix environments

`metewand lock` prepares Nix package or app environments and records their
sources, native lockfiles, realized outputs, runtime closures, and worker
launches in `metewand.lock`. `run --locked` verifies that evidence against the
local store before launching workers. Missing outputs require explicit
preparation; locked runs do not fetch or build software. Resolution preserves
logical identities and binds the concrete environment into resolved identities.

Local flakes use content snapshots, including edits to tracked files, with the
Metewand lockfile excluded. Locked execution still runs trusted local code and
does not enforce network or filesystem isolation.

### uv environments

Native uv environments prepare non-editable Python packages from an
existing `uv.lock` into persistent user data storage. Locked execution verifies
the interpreter, standard library, installed packages, and worker launches
offline without invoking uv. Local dependencies use captured source content.
The evidence does not establish the host native-library closure or an isolated
build toolchain. Version-2 locks distinguish backend evidence while preserving
version-1 Nix records and identities. Native `renv` resolution will follow
adopter needs; Nix remains optional.

### OCI environments

Digest-pinned OCI environments use a local, rootful Linux Docker Engine.
`metewand lock` pulls the native platform and records image and launch evidence.
Checks and locked runs inspect the local Docker content store without fetching
or building software. Verification trusts Docker's content metadata. Version-3
locks add image-scoped launches while preserving version-1 and version-2 records.
Workers use authenticated sockets over host networking. Container CPU and memory
accounting remain unavailable. This backend runs trusted code and does not make
archival isolation claims.

### Apptainer environments

Local SIF environments use Apptainer on Linux. Explicit locking records the image
bytes and native platform without executing image code. Locked checks verify the
existing file without fetching or building software. Version-4 locks add SIF
image evidence while preserving earlier lock readers and identities. Execution
uses the invoking user, authenticated sockets, and host networking. Container
accounting remains unavailable, and this backend runs trusted code without
archival isolation claims.


Isolated archival execution will add enforced
resource, network, and input controls. These stronger guarantees must be
verified before a run can claim them.

## Durable evidence

An attempt records one execution, including failures. An observation records
validated scientific evidence. A valid result that misses a completion target
can produce an observation while its attempt fails. Acceptance and observation
validity remain separate.

The run store combines transactional SQLite coordination with immutable,
verified artifact directories. Publication completes before a terminal success
is reported. Leases and finalization claims coordinate concurrent writers;
recovery verifies published evidence rather than inferring success from the
presence of a directory.

Resume preserves accepted slots and fills eligible unfinished work. Retrying
failed work requires explicit authorization and creates a distinct attempt.
Inspection reads committed evidence without recovering or modifying the store.
Indexes and exports are rebuildable views, not acceptance authorities.

Exports retain individual repetitions, failed attempts, scientific verdicts,
timing scopes, compatibility evidence, and lineage. Statistical analysis and
publication plotting belong downstream of the execution core.

Portable result archives preserve a whole committed snapshot after every started
attempt is terminal. They package versioned records and registered artifacts,
including profile snapshots and traces. Offline verification checks identities,
coordination relationships, inventories, and hashes. Imported collections support
inspection and export without the original run directory or checkout. They remain
read-only and retain the original execution guarantees. Archives preserve the
recorded provenance of external sources and data without gathering their bytes or
provisioning an environment for another execution.
