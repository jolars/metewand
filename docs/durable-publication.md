# Durable observation and attempt publication

`metewand-runtime::publication::finalize_fixed_dataset` publishes the validated
result of the [fixed-dataset runtime path](fixed-dataset-run.md). It first
finalizes an independently readable observation, then finalizes the terminal
attempt that references that observation. It returns typed, identified records
and their absolute directory paths only after both publications are durable.

The caller supplies an existing absolute output root, the resolved observation
and measured attempt slot IDs for the same run, the reserved retry index, and a
canonical object containing contextual provenance. Slot resolution and retry
reservation remain the caller's responsibility. The caller must keep the
private result directories available until publication returns.

```rust
use metewand_runtime::publication::{PublicationRequest, finalize_fixed_dataset};

let published = finalize_fixed_dataset(
    &validated_result,
    PublicationRequest {
        root: output_root,
        attempt_slot,
        observation_slot,
        retry_index: 0,
        provenance_data: context,
    },
)?;
```

The resulting layout is:

```text
observations/<observation-id>/
  observation.json
  metrics.json
  result/                      complete original result tree
  manifest.json
attempts/<attempt-id>/
  attempt.json
  logs/implementation/stdout.log
  logs/implementation/stderr.log
  logs/problem-evaluator/stdout.log
  logs/problem-evaluator/stderr.log
  manifest.json
```

The observation's result reference names `result/`. Its digest is the versioned
[local-tree hash](local-tree-hashing.md), including the original result manifest
and every declared payload file. The metrics reference names `metrics.json` and
hashes its exact original bytes with SHA-256. The observation provenance's
`result_manifest` field locates the original manifest within `result/`, including
when a worker uses a nested or nonstandard manifest name. Both artifacts survive
removal of the private worker output directories.

Result artifact IDs use `identify_canonical<ResultArtifact>` with
`{"tree_hash_version":1,"tree_sha256":"<digest>"}`. Metric artifact IDs use
`identify_canonical<MetricsArtifact>` with `{"sha256":"<digest>"}`. These
resources receive the existing version-1 canonical-resource envelope and
`artifact` domain separator. Observation and attempt IDs use their existing
[typed record identities](identities.md). In particular, an attempt's slot and
retry index determine its ID before observation publication, avoiding a cycle
with the observation's producing-attempt reference.

`metewand-core::run_documents` renders the typed records into the version-1
public observation and attempt envelopes. Documents use canonical JSON without
a trailing newline and are checked against the embedded public schemas before
publication. Timing preserves executor-observed wall time, optional worker
implementation time, and independent evaluator time as separate nanosecond
fields. Unavailable process-tree CPU time and peak RSS are explicit nulls.
Wall-clock attempt timestamps use UTC RFC 3339 with nanosecond precision and do
not contribute to attempt identity.

A valid result with `reached` or `not_required` completion produces an accepted
attempt. A valid result with `not_reached` completion still produces a durable
observation, while the attempt records failure with
`completion_target_not_reached` and retains that observation reference.

## Provenance and logs

Both records include tool, wire-protocol, and execution-semantics versions.
Their provenance data contains:

- `execution`: the development environment class, trusted local executor,
  architecture and OS, exact allowlisted launches and worker identities, SDK
  metadata and reported capabilities, problem and implementation parameters,
  implementation seed, dataset path and tree digest, selected result and metric
  schema paths, timing scope, and unsupported isolation and accounting controls;
- `context`: caller-observed repository, build, environment, and host evidence;
- `result_manifest` and `implementation_statistics`: the original result manifest
  location and worker-reported diagnostic statistics; and
- `logs`: retained byte counts, total drained byte counts, and truncation flags
  for each role and stream.

The caller should represent unavailable optional contextual evidence explicitly
as null, including repository commit, dirty-tree hash, exact build fingerprint,
CPU model, selected cores, memory, kernel, language runtime, BLAS, and thread
settings when those facts are unavailable. This Gate-1 development path records
observed evidence and does not claim comprehensive machine provenance or
stronger environment or isolation guarantees.

Log files preserve the captured byte prefixes exactly, including binary output.
They are scoped by both the containing attempt directory and worker role.
Truncation does not stop pipe draining and never affects protocol traffic.

## Publication and verification

Staging directories have unique names, owner-only access, and the same parent
filesystem as their destination. Publication rechecks private result and metric
snapshots, copies the complete result tree, and verifies the copied tree against
its validation-time digest. Symbolic links, special files, nonnormalized paths,
and superfluous empty directories are rejected.

Each `manifest.json` contains `schema_version: 1`, `complete: true`, and a
bytewise path-sorted list of every other file's path, SHA-256, size, and media
type. JSON files use `application/json`; other files use
`application/octet-stream`. The manifest excludes itself to avoid a recursive
hash. The publisher synchronizes every file and nested directory, writes and
synchronizes the completion manifest last, synchronizes the staging root,
publishes with an atomic no-replace rename, and synchronizes the parent.
Platforms without the required rename operation return an error.

A competing publication never overwrites an existing object, including an empty
destination directory. An existing complete object can be reused only after its
inventory and every file's exact bytes match the staged object. Conflicts and
corruption fail publication. Callers should reserve a new retry index for a new
execution rather than reuse a finalized attempt identity.

If attempt publication fails after observation publication, the complete
observation remains available as an orphan. The function returns an error and
never reports an accepted attempt. This separation also supports later profile
work that must preserve observations after a subsequent failure. The runtime
removes unpublished staging directories on ordinary errors. Process termination
can leave staging directories, which have no finalized identity and are never
returned as complete objects.

`verify_finalized_directory` checks the completion marker and exact file
inventory, rejecting missing, changed, undeclared, duplicated, or unsafe entries.
It verifies directory integrity; scientific validation occurs before publication.
This Gate-1 publisher does not add a metadata database, recovery/import
transactions, or quarantine. The separate [transactional run-store
publisher](run-store-schema.md#transactional-publication) connects leased artifact
publication to SQLite completion; the current CLI does not use it yet. The [`metewand run` command](run-cli.md) composes it with manifest
resolution and execution. A completed orphan alone does not establish attempt
acceptance. `finalize_failed_attempt` uses the same durable boundary to publish
technical failures without creating an observation.

The manifest, public-schema, identity, local-tree-hash, execution-semantics, and
wire-protocol compatibility constants remain at version 1. This implementation
uses the existing contracts without changing their representations. Tests pin
canonical completion-marker bytes and artifact identity vectors, validate the
emitted public envelopes, independently check file hashes and sizes, and cover
nested binary results, truncated logs, missed completion targets, competing
writers, corruption, modified private outputs, and observation survival after
attempt publication failure.
