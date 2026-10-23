# Compatibility versions

Metewand versions its public contracts separately from its package release.
Versionary manages the shared crate SemVer from Conventional Commits. A package
release does not by itself change how readers interpret manifests, schemas,
worker messages, identities, or execution records.

## Gate 3 baseline review

The Gate 3 applicability and profile design extends the planned interfaces. This
review preserves the implemented contracts below; it introduces no fields,
capabilities, or execution behavior. Existing canonical bytes, hashes, identity
vectors, and worker frames remain unchanged.

| Constant | Current value | Contract and extension review |
| --- | --- | --- |
| `MANIFEST_VERSION` | `1` | The strict typed manifest admits `one_shot`, the existing execution policies, and no scientific budget selection. Review this version when admitting applicability, profile policies, implementation bindings, control schemas, or budget values. |
| `PUBLIC_SCHEMA_VERSION` | `1` | Published entry points reject unknown Metewand-owned properties. Extending an existing strict entry point requires a new public schema compatibility version, including updates to its producers and consumers. Publish new decision, trace, and checkpoint schemas with their first execution slice. |
| `WIRE_PROTOCOL_VERSION` | `1` | The implemented work methods are `materialize`, `prepare`, `execute`, `evaluate`, `reset`, and `shutdown`. Adding fields or methods requires a negotiated protocol version and matching schemas, framing vectors, workers, and SDK helpers. |
| `IDENTITY_VERSION` | `1` | Typed identities use the existing kind-separated SHA-256 construction and canonical representations. Review any new kind, dependency, or representation. A new kind can retain this construction version if existing kind representations and bytes stay unchanged; changing an existing representation requires explicit compatibility review and updated vectors. |
| `EXECUTION_SEMANTICS_VERSION` | `2` | Current behavior includes deterministic scheduling and bounded concurrent dispatch. Increment this version when changed execution behavior can affect measurements, applicability, completion, or resolved identities. The design alone does not cause an increment. |

The constants live in `crates/metewand-core/src/lib.rs`, except the wire
version, which lives in `crates/metewand-protocol/src/lib.rs`. Execution
semantics version 2 already predates Gate 3; see the
[scheduling compatibility review](scheduling.md#compatibility-review).
The [measurement provenance schema](execution-measurements.md) continues to
read existing version-1 and version-2 evidence.

`SEED_DERIVATION_VERSION` and `SCHEDULING_POLICY_VERSION` also remain `1`.
Gate 3 must review them if it changes the corresponding transcripts. Package
SemVer, these compatibility versions, and runtime storage versions do not
substitute for one another.

## Current acceptance boundary

The version-1 manifest and standalone observation-policy schema admit only
`one_shot`. Problem contracts require `supported-budgets: ["none"]`.
Applicability selection, profiles, observation-control schemas, scientific
budget schemas and selections, and instrumented measurement settings require
later implementation slices.

The handshake already recognizes `applicability`, `fresh_sequence`, and
`streaming_profile` as capability metadata. This does not make those operations
selectable in a manifest or implement their exchanges. `buffered_profile` is
absent from the version-1 handshake capability set. In particular, the current
`execute` request has only `id`, `method`, and `result_dir`; it rejects
`scientific_budget`, `observation_control`, and instrumentation fields. Its
response does not admit observation controls or buffered traces.

These boundaries are checked in both public schemas and typed parsers. The
negative vectors describe fields that version 1 rejects; they do not freeze the
future syntax for those features.

## Conformance evidence and later slices

The baseline uses the existing byte-level vectors and adds acceptance-boundary
vectors for the design extension:

| Contract | Checked-in evidence |
| --- | --- |
| Canonical values | [Canonical JSON vectors](https://github.com/jolars/metewand/blob/main/fixtures/canonical-json/v1.json), shared with the SDKs. |
| Manifest bytes and hashes | `pins_the_version_one_manifest_hash_bytes` in `crates/metewand-core/tests/manifest_hashing.rs`, plus the default, ordering, and dependency checks in that suite. |
| Public schemas and typed inputs | [Schema conformance cases](https://github.com/jolars/metewand/blob/main/schemas/v1/fixtures/conformance.json) and [Gate 3 extension rejection vectors](https://github.com/jolars/metewand/blob/main/schemas/v1/fixtures/gate3-extensions.json). Each rejection starts from a valid document and changes one field; both its schema and typed parser must reject the result. |
| Worker messages | [Exact SDK exchange frames](https://github.com/jolars/metewand/blob/main/fixtures/worker-protocol/v1/golden.json) and [compatibility vectors](https://github.com/jolars/metewand/blob/main/fixtures/worker-protocol/v1/compatibility.json). Rust decoders and the public worker schema must agree; accepted compatibility frames must round-trip byte for byte. |
| Identity graph | [Complete canonical representations and identities](https://github.com/jolars/metewand/blob/main/fixtures/identities/v1.json), with dependency-mutation tests in `crates/metewand-core/tests/identity_contract.rs`. |
| Execution semantics | The complete identity graph binds execution semantics version 2; [concurrency policy vectors](https://github.com/jolars/metewand/blob/main/fixtures/identities/concurrency-v1.json), [scheduling vectors](https://github.com/jolars/metewand/blob/main/fixtures/scheduling/v1.json), and the runtime and CLI suites cover current dispatch, timing, and worker lifecycles. |

Run the core and protocol conformance tests in the development environment with:

```sh
test-fast
```

Each later slice must document its decision for the five compatibility constants
before introducing fields or capabilities. Add positive and negative vectors
with the first producer and consumer, including exact canonical bytes where
defined. Keep old vectors to verify existing records and messages under their
original versions. Extend the identity dependency tests to show which inputs
invalidate decisions, logical slots, resolved executions, and observations.
Update the affected SDK conformance cases, durable records, inspection, exports,
and user documentation in that same slice.

The [identity reference](identities.md) defines the existing distinction between
logical and resolved records. Applicability evidence, observation controls,
scientific budgets, environment fingerprints, and exact worker launch identities
must retain their separate meanings as those slices extend the graph.
