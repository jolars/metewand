# Reference

These documents describe Metewand's implemented interfaces, compatibility
contracts, and runtime behavior. Start with the [getting started
guide](../guide/getting-started.md) for a complete first run.

## Commands and workflows

[Create a benchmark](authoring-cli.md), [check and plan it](read-only-cli.md),
[verify its workers](worker-checks.md), and [run it](run-cli.md). After
execution, [inspect its status and identities](inspection-cli.md) or [export
results](exports.md) for analysis. All commands provide [versioned machine
output](machine-output.md).

## Contracts and planning

[Problem contracts](problem-contracts.md) define validity and independent
evaluation. [Public schemas](public-schemas.md), [canonical
JSON](canonical-json.md), and [typed identities](identities.md) fix the
language-neutral representation. [Configuration
expansion](configuration-expansion.md), [logical planning](logical-planning.md),
and [scheduling](scheduling.md) define how a manifest becomes an ordered set of
attempts.

[Compatibility versions](compatibility-versions.md) records the current contract
versions, the pre-MVP stability policy, and verification guidance for extensions.

## Sources, execution, and storage

[Source resolution](source-resolution.md) and the [artifact
store](artifact-store.md) connect declared inputs to verified content. The
runtime records [timing envelopes](fixed-dataset-run.md),
[measurements](execution-measurements.md), and [execution
controls](execution-controls.md). [Durable publication](durable-publication.md)
and the [run store](run-store-schema.md) preserve completed results and support
explicit recovery.

## Worker protocol and SDKs

The [worker protocol](worker-protocol.md) defines communication between Metewand
and worker processes. The [Python SDK](python-sdk.md) and [R SDK](r-sdk.md)
provide callback dispatch, serialization, output helpers, and structured errors.
[Shared conformance suites](sdk-conformance.md) check their compatibility with
the Rust implementation.

The sidebar lists the complete reference. The repository's
[design](https://github.com/jolars/metewand/blob/main/DESIGN.md) also describes
planned interfaces; the
[roadmap](https://github.com/jolars/metewand/blob/main/TODO.md) tracks
implementation progress.
