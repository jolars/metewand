# Gate 1 domain records

`metewand-core::records` defines the data carried through the Gate 1 benchmark
lifecycle. These records are separate from the authored TOML types: manifest
types retain source-facing syntax and optional fields, while domain records
contain resolved references, canonical parameter values, and defaulted policies
ready for identity assignment and planning.

The graph has four boundaries:

```text
definitions ──> configurations ──> logical candidate ──> one-shot specification
      │                │                                      │
      │                └──> dataset instance ──> problem instance
      │                                                       │
      └──> environment resolution and exact worker launches ──┤
                                                              v
                    logical observation/attempt slots ──> resolved slots
                                                              │
                                                              v
                                              attempts and observations
```

Each arrow is represented by a `RecordId<T>` whose marker type names the exact
record or resource at the other end. Rust therefore rejects a dataset-definition
identity where a problem-definition identity is required. `RecordId<T>` stores
the 32-byte digest needed by the graph and renders the public
`mw1-<kind>-<sha256>` spelling. The [identity construction
contract](identities.md) assigns IDs from explicit versioned canonical
representations. `IdentifiedRecord<T>` pairs the resulting identity with its
record without putting a record's own identity into its hash input.

`CapabilityReport<T>` keeps implementation, worker, and executor capability
namespaces distinct while pairing each capability with `CapabilityEvidence`.
The evidence is `declared`, `verified_now`, or `previously_verified`.
Declarations belong to identity-bearing definition records; the evidence state
describes what the current operation knows and is not itself an identity input.

## Definitions, configurations, and instances

The identity-ready definition records cover datasets, problems,
implementations, software environments, workers, execution policies, and
observation policies.

- `DatasetDefinitionRecord` distinguishes the built-in unit dataset, fixed
  repository data, and generated data. It retains parameter and output schema
  identities, canonical defaults, source bundles, remote-source digests, and
  the materializer definition where applicable.
- `ProblemDefinitionRecord` binds a validated contract to its independent
  evaluator.
- `ImplementationDefinitionRecord` replaces manifest problem names with exact
  problem-definition identities and retains its worker, schemas, defaults, and
  capabilities.
- `EnvironmentDefinitionRecord` retains the backend-specific unresolved
  definition. `WorkerDefinitionRecord` refers to it and distinguishes
  interpreted entrypoints from raw commands.
- `ExecutionPolicyRecord` and `ObservationPolicyRecord` represent policies
  after built-in defaults have been applied.

Dataset, problem, and implementation configuration records pair those
definitions with validated canonical parameters. Dataset configurations also
carry the exposed seed and complete derivation digest. A dataset instance binds
its configuration to an immutable artifact, and a problem instance binds
exactly one such dataset—including the built-in unit instance—to a configured
problem.

## Logical and resolved records

`LogicalCandidateRecord` contains only configuration and policy identities. It
can therefore exist before downloads, materialization, environment resolution,
or worker launch. The corresponding logical-plan wrapper reports each selected
implementation capability with declaration evidence only. An applicable
candidate produces an
`OneShotLogicalSpecificationRecord` for each implementation repetition. That
record owns the implementation seed and Gate 1 `none` scientific budget.

Each measurement repetition has one `LogicalObservationSlotRecord` and one
measured `LogicalAttemptSlotRecord`. Warm-ups are separate attempt slots with a
typed `Warmup` role and never masquerade as measured observations. Every
attempt slot retains its deterministic scheduling priority.

Resolution adds a materialized problem instance, resolved source bundles,
environment fingerprints, exact launch specifications, the executor, the wire
protocol version, and the execution-semantics version. Logical identities remain
unchanged. Observation and attempt slots then receive corresponding resolved
records that point to this complete dependency set.

## Attempts and observations

A `RunObservationRecord` represents only a schema-valid result accepted as
valid by the independent evaluator. It carries typed result and metric artifact
references, executor-observed timing, completion status, and provenance.

`RunAttemptOutcome` preserves the one-shot cardinality rules in its shape:

- `Accepted` contains exactly one observation identity; and
- `Failed` contains a structured diagnostic and may retain one valid
  observation—for example, when a valid result did not reach the contract's
  one-shot completion target.

Every retry is a separate `RunAttemptRecord` with its resolved slot and retry
index. Wall-clock timestamps and observed provenance remain on attempts and
observations; they are not inputs to logical or resolved identities.

The records module does not parse manifests, expand parameter grids, resolve
runtime resources, perform execution, or serialize the public attempt and
observation schemas. The separate seed module derives the seed records consumed
here; the remaining operations consume these records in later roadmap slices.
