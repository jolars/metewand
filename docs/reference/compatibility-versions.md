# Compatibility versions

Metewand versions public formats separately from package releases. Versionary
manages the shared crate SemVer from Conventional Commits. Readers use the
version declared by a format rather than infer it from the package release.

## Pre-MVP development policy

Authoring syntax and the new profile APIs are under active development.
They may evolve together with their current producers and consumers before the
local MVP. Pin a Metewand revision for retained benchmark runs. This does not
promise support for every intermediate development syntax or message shape.

Preserve supported persisted records and existing canonical identity meanings.
A change must not silently reinterpret historical results, mix timing scopes,
or reuse an identity for different scientific or execution semantics. Use an
affected format or semantics version, a migration, or an explicit unsupported
version diagnostic when compatibility requires it. Protocol extensions need
compatible capability negotiation or a protocol version supported by both peers.

Consider only the boundaries a change actually affects. Record a consequential
version decision briefly in the change description and update the relevant
tests. There is no requirement for a separate compatibility review document,
a table of decisions for unchanged constants, or a new conformance suite before
implementation begins.

Publish schemas for externally consumed inputs, wire messages, durable records,
and exports. Internal helper types stay private; nested records normally inherit
their enclosing format's version. Prefer deriving new schemas from authoritative
boundary types where practical. Do not make conversion of the existing schema
catalog a prerequisite for the MVP.

## Current versions

| Constant | Value | Meaning |
| --- | --- | --- |
| `MANIFEST_VERSION` | `1` | Authored benchmark definitions. |
| `PUBLIC_SCHEMA_VERSION` | `1` | Published machine-readable record shapes. |
| `WIRE_PROTOCOL_VERSION` | `1` | Worker negotiation and exchanges. |
| `IDENTITY_VERSION` | `1` | Typed canonical identity representations. |
| `EXECUTION_SEMANTICS_VERSION` | `2` | Execution behavior that affects measurements or resolved identities. |

The constants live in `crates/metewand-core/src/lib.rs`, except the wire
version in `crates/metewand-protocol/src/lib.rs`.
`SEED_DERIVATION_VERSION` and `SCHEDULING_POLICY_VERSION` remain `1`;
change them only when the corresponding transcripts or policies change.
The [measurement provenance schema](execution-measurements.md) continues to
read existing version-1 and version-2 evidence. This policy update changes no
constants, schemas, or accepted inputs.

## Current input boundary

The manifest and standalone observation-policy schema currently admit only
`one_shot`. Problem contracts require `supported-budgets: ["none"]`.
Inline contracts and schemas are supported. Result and metric envelopes may
omit `schema` to use the selected contract's schema. These additions retain the
unreleased version-1 authoring and public-schema boundaries: existing canonical
file declarations and persisted records keep their representations. Inline
schemas inherit the dialect before hashing, and schema identities include their
resolved dependencies. Moving a schema from a file to an inline declaration may
change its path-bearing identity; see [schema normalization](schema-validation.md#inline-declarations).

Compact dataset, problem, and implementation `parameters`, plus contract
`result` and `metrics`, add authoring syntax within the unreleased version-1
manifest, problem contracts, and public schemas. They lower into the existing
inline schemas and literal default fields, preserving canonical representations
and component identities for equivalent explicit declarations. Declaration
grids remain expansion selections, separate from schemas and definition
identities. Nonempty inline declaration grids also contribute to the manifest
hash; existing declarations retain their canonical bytes. Plans report each
candidate's resolved parameter mappings. This changes no wire messages,
execution semantics, or persisted record formats.

Omitted parameter declarations now normalize to a closed empty-object schema.
Previously schema-less generated datasets and implementations therefore acquire
schema identities and new definition identities when replanned. Existing
persisted records retain their original meanings and remain readable; this does
not change canonical identity transcripts or the run-store format. Equivalent
omitted and explicit defaults share the normalized authoring identity path.
Local-environment inference, nested policy versions, and repetition defaults
resolve before hashing. Selected policies must now explicitly name their timing
scope and primary clock. Existing authoring files that omitted those choices
must add them. These pre-MVP authoring changes retain compatibility version 1;
wire messages, persisted records, and execution semantics are unchanged.

Flat problem declarations also retain authoring and public-schema version 1.
They inherit the contract version and mapping name, then use the same typed
contract, canonical manifest bytes, schema resources, and component identities
as equivalent wrapped inline declarations. Referenced and explicit inline
contracts remain supported. Combining either with flat contract fields is an
error. This changes no persisted records, wire messages, or execution semantics.

Plain scalar experiment and case parameter entries retain authoring and
public-schema version 1. They lower to the existing literal `value` axes before
hashing and expansion, so equivalent explicit axes keep their canonical bytes,
plans, and component identities. YAML source locations are diagnostic metadata
and do not enter identities or persisted records.

Optional family semantics schemas, applicability
selection, profiles, scientific budgets, and instrumentation settings remain
planned implementation work.

The handshake recognizes `applicability`, `fresh_sequence`, and
`streaming_profile` as capability metadata, but their work exchanges are not
implemented. `buffered_profile` is absent from the current capability set.
The current `execute` request admits only `id`, `method`, and
`result_dir`.

Strict schemas and typed parsers must agree on what the implementation accepts.
The [extension rejection vectors](https://github.com/jolars/metewand/blob/main/schemas/v1/fixtures/gate3-extensions.json)
test today's boundary. Add positive coverage at the supported version when a
feature is implemented. Retain old rejections where an older format remains
supported; these vectors do not freeze future development interfaces.

## Verification

Keep exact golden vectors for [canonical values](canonical-json.md),
[identities](identities.md), and [seed derivation](seeds.md). Use focused protocol,
schema, timing, and recovery tests for affected behavior. Extend the SDK
conformance cases for callbacks actually added; every language need not
implement every profile callback before an adopter can use one.

The core and protocol checks run with `test-fast` in the development
environment. See [SDK conformance](sdk-conformance.md) for shared vectors.
