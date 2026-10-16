# Dataset materialization

`metewand-runtime::dataset_materialization::materialize_dataset` executes a
benchmark-owned generator or transformation through the version-1 materializer
protocol. The operation returns an owned private dataset, its validated manifest,
a construction identity, a separate output tree hash, bounded process logs, and
canonical provenance. Errors remove partial output. Durable storage, leases,
and derivation-to-output cache mappings are separate runtime operations.

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
once per run. The download limit is 1 GiB of raw bytes, and the materialization
phase timeout also bounds the HTTP request. `check` and `plan` never acquire
sources or execute workers. The [generated dataset example](../examples/generated-dataset)
provides a complete raw-protocol generator, implementation, and evaluator.

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
