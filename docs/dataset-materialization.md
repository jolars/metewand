# Dataset materialization

`metewand-runtime::dataset_materialization::materialize_dataset` executes a
benchmark-owned generator or transformation through the version-1 materializer
protocol. The operation returns an owned private dataset, its validated manifest,
a construction identity, a separate output tree hash, bounded process logs, and
canonical provenance. Errors remove partial output. The
[artifact store](artifact-store.md) provides durable publication, leases, derivation-to-output mappings, and private
attempt views as separate runtime operations.

## Execution and validation

The caller supplies the checked dataset configuration, resolved materializer
launch and worker record, offline schema catalog, output schema, operational
phase timeouts, log limits, and a private staging parent. Source and staging directory paths
must be absolute UTF-8 paths. The worker record binds the
checked source bundle, resolved environment, and normalized launch. Callers must
retain those checked inputs and verify that they remain unchanged around the
operation, as the manifest-driven runner does.

A pure generator receives an empty private `source_dir`. A transformation
receives a `ResolvedSource` from the [source-resolution boundary](source-resolution.md),
including repository files, pinned downloads, or an external provider export.
Pinned downloads retain their original bytes in `source`; any scientific
format conversion belongs to the materializer. Archive extraction is an
explicit bounded operation rather than an inference from a URL suffix.

Metewand performs the materializer handshake using the resolved worker identity,
sends the complete canonical dataset parameters and planned dataset seed, and
requests shutdown after materialization. Each phase has its own deadline. It
then validates the public artifact envelope, the complete payload inventory and
hashes, and the dataset output schema. It writes the manifest as canonical JSON,
without changing scientific payload bytes. Sources are checked for mutation,
and the resulting tree receives a separate versioned local-tree hash. A later
consumer calls `MaterializedDataset::verify()` before use.

`metewand run` supports local generators and transformations with pinned HTTP(S)
inputs. It preflights every selected launch and fixed dataset before acquisition
or materializer execution and constructs each distinct dataset configuration
once per run or reuses its verified construction from an explicit `--cache-dir`.
The download limit is 1 GiB of raw bytes, and the materialization phase timeout
also bounds the HTTP request. `check` and `plan` never acquire
sources or execute workers. The [generated dataset example](../examples/generated-dataset)
provides a complete raw-protocol generator, implementation, and evaluator.

## Materialization and implementation preparation

Shared materialization constructs the input to a comparison. Every selected
implementation receives a private view of that same immutable dataset instance.
Implementation preparation adapts that input to one implementation without
changing the instance or the requested computation.

| Work | Owner and boundary |
| --- | --- |
| Generation, fold selection, or scientific preprocessing shared by the comparison | The benchmark-owned materializer uses dataset parameters and the dataset seed before measurement. |
| A transformation into a different dataset schema | An explicit materializer declares the output schema accepted by the problem contract. Planning never infers a conversion from matching fields. |
| Loading JSON into a native array, packing integers, or converting an array layout for one library | The implementation performs the conversion in `prepare` or `execute`, preserving the contract's values and conventions. |
| A change to the target, selected data, standardization, or result interpretation | The benchmark author represents the change in the dataset or problem configuration, even when a library exposes it as an implementation option. |

The materializer request contains only the verified source directory, canonical
dataset parameters, dataset seed, and assigned output directory. It receives no
implementation configuration or implementation seed. Changing the selected
implementation or its preparation does not create another shared dataset
construction. A changed materializer source bundle does change the derivation,
including when implementation and materializer code share that bundle.

An implementation must not write converted files into `dataset_dir`, even
though that view is private to the attempt. It keeps native data in memory or
its private working directory. Metewand checks the dataset around every worker
operation; adding a conversion file or rewriting an input invalidates the
attempt with `dataset_mutation` before execution continues. Local enforcement
remains best-effort, as described in [artifact storage](artifact-store.md).

### Work outside the timed scope

Dataset acquisition, shared construction, and validation precede measured
execution. Moving implementation-specific preprocessing into materialization
would exclude that implementation's cost and violate this boundary. A shared
scientific transformation must be part of the declared dataset recipe and the
problem's data conventions.

The problem contract defines allowed implementation preparation in its
schema-validated, hashed family semantics. For example, the generated dataset
contract declares:

```toml
[semantics.preprocessing]
shared_materialization = "canonical_integer_json"
implementation_conversion = "preserve_integer_values_and_order"
allowed_outside_timed_scope = []
```

These field names belong to this problem family. Metewand validates and hashes
them; the benchmark author, worker conformance tests, and independent evaluator
establish their scientific meaning. Metewand does not infer permission from a
method name or discover hidden preprocessing in arbitrary worker code.

The selected policy determines the actual timer. `prepare_and_execute` includes
all of `prepare`, `execute`, and canonical result writing, even if the contract
would permit some preparation outside another scope. Dataset integrity checks
between prepare and execute also remain inside that continuous timer. Worker
startup, result validation, evaluation, metrics validation, and shutdown remain
outside it and are identified in execution provenance. `cold_end_to_end` also
includes worker launch. `execute_only` permits untimed preparation only under
precise contract rules and must be reported as an operation-scope benchmark.
Readers must not implicitly pool measurements from different scopes. The
current local CLI implements only `prepare_and_execute` and rejects other scopes
before worker launch or output creation, even if the contract permits them.

The [generated dataset example](../examples/generated-dataset) exercises this
boundary with list and packed-integer implementations. Its
[conversion vector](../examples/generated-dataset/fixtures/preparation.json)
specifies the exact native values and expected result and metrics. The
[CLI conformance tests](../crates/metewand-cli/tests/dataset_preparation.rs)
verify one shared construction, identical input bytes and problem instances,
separate attempt views, exact conversions, preparation on every attempt,
conversion and result writing inside the timer, and shared construction outside
every timer. They also verify mutation rejection, incompatible schema and timing
rejection before launch, and the effect of preparation rules on contract
identity without changing the dataset derivation. These cases clarify existing
version-1 semantics; compatibility constants and public schemas are unchanged.

## External provider evidence

External selection remains explicit through `SourceRequest::External` and a
caller-supplied `SourceAcquirer`. No provider cache is treated as owned storage.
Before using an external source in a derivation, Metewand requires the backend's
acquisition receipt to contain:

```json
{
  "external_dataset": {
    "resolved_dataset_version": "dataset-release-7",
    "decoding_recipe": {"format": "json", "decoder_version": 1},
    "decoded_values": "provider_verified"
  }
}
```

The resolved version must be nonempty, and the decoding recipe must be an
object. `decoded_values` accepts `not_verified` or `provider_verified`. Missing
or invalid evidence rejects materialization. `provider_verified` preserves the
provider's claim; Metewand's independently recorded `raw_bytes_verified: true`
only establishes byte integrity. The provider must validate immutable selectors
and associate the exported inventory and decoding evidence with that selection.
The receipt keeps the provider name, exact selector, resolved dataset version,
recipe, status, complete file sizes and hashes, and original acquisition receipt.
External-provider manifest syntax remains outside the current CLI contract.

## Contract-owned binary validation

JSON Schema validates the dataset manifest, not opaque binary payloads. A runtime
caller can supply `ContractDatasetValidator` when its selected problem contract
requires additional checks. The hook carries that contract's identity, a
resolved checker launch and worker record, and an independent operational
timeout. The caller owns the association between the selected contract and its
checker; it must never silently omit a required checker. The current manifest
format does not declare this hook, so it is selected explicitly by runtime callers.

The checker runs as a separate process with an allowlisted environment and a
private working directory. Metewand appends two arguments:

```text
--metewand-dataset-validation-v1 <canonical-request-json>
```

The request contains `version: 1`, `contract_id`, the absolute `dataset_dir`, and
the output-relative `manifest` path. The checker opens the indicated manifest
and payloads, validates their scientific binary representation, and exits zero
to accept. A nonzero status or timeout rejects the entire dataset. Standard
output and error are bounded diagnostic logs. Output mutations fail validation.
The receipt records the contract, checker identity, command version, and success.
This standalone command contract has its own compatibility version and schema;
it does not add a worker wire message or change wire protocol version 1.

## Identities and compatibility

`DatasetDerivationRecord` introduces the `dataset-derivation` kind under the
existing version-1 identity algorithm. Its canonical transcript binds the
dataset definition, verified sources and tree-hash versions, external selectors
and decoding evidence, resolved materializer code and environment, normalized
launch, canonical parameters, and complete dataset seed record. Host staging
paths, acquisition timestamps, process logs, SDK reports, and observed output
hashes do not enter the construction identity. The output hash remains separate,
so nondeterministic output cannot disguise itself as a different recipe.

The canonical transcript vector lives in
[fixtures/identities/dataset-derivation-v1.json](../fixtures/identities/dataset-derivation-v1.json).
Existing identity representations are unchanged, so `IDENTITY_VERSION` stays 1.
The new `dataset-materialization` receipt and `dataset-validation-request`
command request have version-1 public schemas and positive and negative
conformance cases. Manifest, source-resolution, public-schema, wire-protocol,
and execution-semantics versions retain their existing values. No timing scope
changes: dataset construction and validation occur before measured execution.
