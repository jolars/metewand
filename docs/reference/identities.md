# Version-1 identities

Metewand assigns stable identities to canonical resources and typed domain
records. An identity has this canonical lowercase spelling:

```text
mw1-<kind>-<64 lowercase hexadecimal SHA-256 digits>
```

The digest is computed as:

```text
SHA-256(UTF-8(kind) || NUL || canonical-representation)
```

The representation is canonical JSON in Metewand's
[restricted version-1 domain](canonical-json.md). It never depends on a Rust
serialization layout. Record representations are objects containing a numeric
`version` field with value `1`; canonical resources use
`{"version":1,"value":<resource>}`. The `version` is the identity
representation version, not a package version.

`metewand-core::identity` exposes:

- `identify_record`, which returns an `IdentifiedRecord<T>`;
- `record_id`, which hashes a borrowed identity-ready record;
- `canonical_identity_bytes`, which exposes the exact compatibility bytes;
- `run_attempt_id`, which identifies a reserved slot and retry before its
  terminal record exists, using the existing attempt representation;
- `identify_canonical`, which identifies a typed, validated canonical
  resource; and
- typed `Display`, `FromStr`, `Serialize`, and `Deserialize` implementations
  for `RecordId<T>`.

Programmatically constructed records are checked again at the identity
boundary. An integer outside the safe JSON range, for example, is rejected
instead of receiving an implementation-specific hash.

The read-only repository loader constructs canonical resources before calling
this generic identity API. A repository schema resource contains its logical
path, canonical document, and the path and canonical document of every
repository-local schema reachable through `$ref`; cycles are deduplicated.
A validated problem-contract resource replaces schema paths with those resolved
schema identities and binds each reference-case path to a content identity.
Source-bundle resources use the deterministic filesystem inventory documented
in [Source bundles](source-bundles.md). These resolved resource representations
ensure that a transitive schema, fixture, or worker-source change invalidates
the definition and logical identities that depend on it.

## Kind registry

Each Rust marker or record type has one stable kind. Artifact roles remain
distinct Rust types but deliberately share the `artifact` content kind, so
identical artifact contents have one content address.

| Kind | Rust type or marker |
| --- | --- |
| `schema` | `SchemaResource` |
| `problem-contract` | `ProblemContractResource` |
| `source-bundle` | `SourceBundleResource` |
| `artifact` | `SourceBundleArtifact`, `DatasetArtifact`, `ResultArtifact`, `MetricsArtifact` |
| `environment-definition` | `EnvironmentDefinitionRecord` |
| `worker-definition` | `WorkerDefinitionRecord` |
| `dataset-definition` | `DatasetDefinitionRecord` |
| `problem-definition` | `ProblemDefinitionRecord` |
| `implementation-definition` | `ImplementationDefinitionRecord` |
| `execution-policy` | `ExecutionPolicyRecord` |
| `observation-policy` | `ObservationPolicyRecord` |
| `dataset-configuration` | `DatasetConfigurationRecord` |
| `problem-configuration` | `ProblemConfigurationRecord` |
| `implementation-configuration` | `ImplementationConfigurationRecord` |
| `dataset-derivation` | `DatasetDerivationRecord` |
| `dataset-instance` | `DatasetInstanceRecord` |
| `problem-instance` | `ProblemInstanceRecord` |
| `logical-candidate` | `LogicalCandidateRecord` |
| `logical-specification` | `OneShotLogicalSpecificationRecord` |
| `logical-observation-slot` | `LogicalObservationSlotRecord` |
| `logical-attempt-slot` | `LogicalAttemptSlotRecord` |
| `resolved-environment` | `ResolvedEnvironmentRecord` |
| `resolved-launch` | `ResolvedLaunchRecord` |
| `executor-definition` | `ExecutorDefinitionRecord` |
| `resolved-worker` | `ResolvedWorkerRecord` |
| `applicability` | `ApplicabilityDecisionRecord` |
| `resolved-specification` | `ResolvedOneShotSpecificationRecord` |
| `resolved-observation-slot` | `ResolvedObservationSlotRecord` |
| `resolved-attempt-slot` | `ResolvedAttemptSlotRecord` |
| `observation` | `RunObservationRecord` |
| `attempt` | `RunAttemptRecord` |
| `diagnostic` | `inspection::DiagnosticResource` |

Parsing an identity through `RecordId<T>` verifies the kind associated with
`T`; a schema ID therefore cannot be parsed as an environment-definition ID.

Persisted failure diagnostics use the canonical resource value
`{"code":...,"message":...,"details":...}`; missing details are null. Attempt
IDs, paths to stored records, and navigation links are excluded. The
[diagnostic vectors](https://github.com/jolars/metewand/blob/main/fixtures/identities/diagnostics-v1.json) define exact
canonical bytes and IDs. This additive kind retains `IDENTITY_VERSION = 1` and
does not change existing identity transcripts. See [inspection](inspection-cli.md)
for lookup and evidence availability.

## Representation rules

All record fields listed below accompany `"version":1` in the top-level
object. Optional values are represented explicitly as `null`. Names, paths,
enumerations, and other strings use their validated public spellings. Ordered
arguments and other semantic sequences remain arrays. Sets—currently an
implementation's declared problem contracts and capabilities—are sorted before
canonicalization. JSON objects, including environment-variable maps, are
ordered by canonical JSON rather than insertion order.

Dependencies are always full typed identity strings. A `RecordId<T>` is never
encoded as bare digest bytes or debug output. This makes each parent a Merkle
node: replacing a dependency with its newly computed ID changes every parent
that transitively names it without coupling unrelated records.

The version-1 record fields are:

| Record | Identity fields |
| --- | --- |
| Environment definition | `name`, and backend-specific `definition` |
| Worker definition | runner-specific `launch`, ordered `args`, environment ID, and optional protocol transport |
| Dataset definition | `name`, optional parameter-schema ID, optional canonical defaults, output-schema ID, and kind-specific definition |
| Problem definition | `name`, contract ID, optional canonical defaults, and evaluator definition |
| Implementation definition | `name`, worker definition, sorted problem-definition IDs, optional parameter-schema ID, optional canonical defaults, and sorted capabilities |
| Execution policy | `name` and every fully defaulted resource, isolation, timing, and ordering field |
| Observation policy | `name` and observation `kind` |
| Dataset configuration | definition ID, canonical parameters, and the numeric and full-digest seed record |
| Problem configuration | definition ID and canonical parameters |
| Implementation configuration | definition ID, canonical parameters, and selected environment-definition ID |
| Dataset instance | configuration ID, artifact ID, and output-schema ID |
| Problem instance | dataset-instance ID and problem-configuration ID |
| Logical candidate | the three configuration IDs and the two policy IDs |
| Logical specification | `one_shot` kind, candidate ID, scientific budget, implementation-repetition index, and implementation seed record |
| Logical observation slot | logical-specification ID and measurement index |
| Logical attempt slot | logical-specification ID, typed warm-up or measured role, and scheduling-priority digest |
| Resolved environment | environment-definition ID and canonical backend fingerprint |
| Resolved launch | resolved-environment ID, exact program, ordered arguments, allowlisted environment map, and working-directory convention |
| Executor definition | executor kind and canonical configuration |
| Resolved worker | optional source-bundle artifact ID, resolved-environment ID, and resolved-launch ID |
| Applicability decision | candidate ID, outcome, stable code, reason, exact checked inputs, resolved worker and environment evidence, controls, and optional details |
| Resolved specification | optional checked applicability ID, `one_shot` kind, logical-specification ID, problem-instance ID, implementation and evaluator resolutions, executor ID, wire-protocol version, and execution-semantics version |
| Resolved observation slot | logical-slot ID and resolved-specification ID |
| Resolved attempt slot | logical-slot ID and resolved-specification ID |
| Observation | resolved-slot ID, producing-attempt ID, result and metric artifact references, timings, and completion status |
| Attempt | resolved-slot ID and retry index |

A checked applicability decision is a dependency of the resolved specification.
Changing its inputs, response, worker, environment, or observed controls changes
that specification and its resolved attempt slots, observation slots, attempts,
and observations. The logical candidate and logical slots remain stable when
only the checked evidence changes. During resume, an accepted attempt under the
previous decision cannot satisfy a slot under the new decision.

Nested definitions use the same field representation as their standalone
record, without a second nested `version`. Raw SHA-256 values use 64 lowercase
hexadecimal digits. Durations use an object with integral `seconds` and
`nanoseconds` fields, which avoids floating-point unit conversions.

The attempt identity intentionally names the execution position rather than its
terminal state. Metewand needs that ID before an observation can name its
producing attempt, while an attempt outcome may itself name that observation.
Including both edges would create a cyclic hash definition. Outcome, timestamps,
diagnostics, and provenance remain in immutable attempt and observation records,
but provenance-only values do not alter their identities. This also ensures that
opaque provenance cannot introduce a workspace, staging, cache, or ephemeral
absolute path into an identity. Wall-clock timestamps are never identity inputs.

## Conformance vectors

[`fixtures/identities/v1.json`](https://github.com/jolars/metewand/blob/main/fixtures/identities/v1.json) pins the
canonical representation, kind, and identity of every version-1 resource and
record in one complete dependency graph. The fixture is language-neutral so
each SDK can verify the same bytes and digests.

[`fixtures/identities/applicability-v1.json`](https://github.com/jolars/metewand/blob/main/fixtures/identities/applicability-v1.json)
adds exact bytes and identities for a checked decision and every dependent
execution record. Unconditional runs retain the original vectors.

The Rust conformance tests rebuild that graph with object members, set-valued
problem contracts, and environment variables supplied in different orders and
require every identity to remain unchanged. They also change each graph node in
turn and require exactly its transitive dependents to change. The manifest and
local-tree suites separately prove that an unrelated experiment and filesystem
enumeration order do not perturb existing component identities.

## Example

The fully defaulted execution policy in the Rust conformance test has these
exact canonical bytes:

```json
{"cpus":1,"enforcement":"best_effort","max_concurrency":1,"memory":null,"name":"default","network":false,"primary_time":"timed_wall_time","run_order":"sequential","threads":null,"timeout":null,"timing_scope":"prepare_and_execute","version":1,"warmup_runs":0,"worker_reuse":false}
```

With the `execution-policy` domain separator, its identity is:

```text
mw1-execution-policy-a7cb0d75d97d065c897a3858debfb8cea2769abb7ab7d5cb80eda49eeffbf486
```

Changing any representation or kind is a compatibility change and requires an
explicit review of `IDENTITY_VERSION`.

Dataset construction uses the new `dataset-derivation` kind without changing
existing transcripts. See [Dataset materialization](dataset-materialization.md)
for its input fields and exact conformance vector. Output hashes are recorded
separately and never participate in the construction identity.

The concurrency bound is an explicit execution-policy member, including its
serial default of `1`. [Scheduling](scheduling.md#compatibility-review) documents
the expanded policy representation, execution semantics version 2, and the
review of the compatibility constants.
