# Datamonger sources

`metewand-runtime::datamonger::DatamongerAcquirer` is an optional source backend.
It runs Datamonger's public Python client in a separate process, exports verified
raw artifacts, and returns the existing `ResolvedSource` type. Python and
Datamonger are required only when the caller explicitly selects this backend.
The benchmark author supplies the materializer that turns those sources into a
standard Metewand dataset.

The [runnable example](../examples/datamonger) resolves `uci:iris@1`, materializes
one dataset instance, and compares two implementations using independent private
views. The current manifest and CLI do not select external providers. Runtime
callers select this adapter directly; `check` and `plan` remain read-only and
never acquire sources.

## Pin the selection

Pass `SourceRequest::External` with provider `datamonger` and a canonical selector:

```json
{
  "version": 1,
  "registry": {
    "schema_version": 1,
    "release": "2026.09",
    "index_sha256": "4c1676acbd8c1164d900161605b60da5b04503e633620230185254a7beff7d61",
    "index_url": "https://github.com/jolars/datamonger/releases/download/registry-2026.09/index.json"
  },
  "dataset": { "source": "uci", "name": "iris", "version": "1" },
  "verification": "decoded"
}
```

The registry release and SHA-256 fix the exact index bytes. The dataset version
is mandatory; source and name are separate unqualified identifiers. Missing
pins, unsupported versions, unknown selector fields, and floating registry
aliases fail before the process launches. Index URLs must use HTTP(S) without
credentials or fragments. The adapter passes the full registry and dataset
selection to every Datamonger call, so project selectors, session defaults, and
registry discovery cannot replace it. Datamonger verifies index bytes against
the pin and resolves exactly the requested version.

The registry pin is relative to the channel that supplied it. See
[Datamonger's trust model](https://github.com/jolars/datamonger/blob/main/TRUST.md)
for the guarantees of its selectors, artifacts, and canonical decoded digests.

## Choose the verification claim

| Selection | Provider operation | Materialization evidence |
| --- | --- | --- |
| `artifact` | `data_info` and `fetch_artifact` verify the index and every exported artifact's size and raw SHA-256. No decoding occurs. | `decoded_values: "not_verified"`; canonical verification fields are null. |
| `decoded` | The same artifact operations, followed by `fetch_data(verify_decoded=True, return_info=True)`. Datamonger verifies the representation's component shapes and canonical decoded digest. | `decoded_values: "provider_verified"`, with canonical-form version and digest. |

The adapter checks the returned registry, dataset ID, version, verification
level, and decoded artifact selection against the request. A decoded response
must match a SHA-256 verification record in the selected representation. Any
verification failure rejects resolution; the adapter never downgrades the
request. Both modes export the same raw bytes. Datamonger's Rust CLI performs
artifact verification only; this adapter uses the Python client to support the
additional decoded verification explicitly. See the
[upstream Python guide](https://github.com/jolars/datamonger/blob/main/packages/python/README.md).

Metewand independently verifies the complete exported inventory through
`resolve_source`. Its `raw_bytes_verified: true` materialization evidence remains
distinct from the provider's decoded-value claim. Provider verification applies
to the registered representation, not automatically to a benchmark author's
later scientific transformation.

## Runtime use and ownership

Construct `DatamongerAcquirer::new(python, cache_dir, timeout, max_bytes, offline)`
and pass it as the explicit acquirer to `resolve_source`. Interpreter and cache
paths must be absolute. Preserve a virtual environment's interpreter symlink:
resolving it to the base interpreter can discard its installed packages. An
interpreter wrapper may supply native library dependencies when the environment
requires them, including on NixOS.

The bridge starts isolated Python with `-I`, a private working directory, null
stdin, and no inherited environment variables except its process transport
descriptors. It captures at most 8 KiB from each standard stream. A positive
deadline bounds acquisition, and `max_bytes` bounds the sum of the registry's
pinned artifact sizes before fetching. Each exported copy is also bounded by
its pinned size. Response receipts are limited to 1 MiB. A failure removes the
private source and command staging directories.

Each artifact is copied to `artifacts/NAME` in the assigned source directory.
The adapter exports all artifacts declared by the selected dataset; decoded
verification covers those selected by its representation's input roles. These
operations use public `fetch_artifact` paths without moving, linking, chmodding,
or deleting provider cache objects. An independent hash check rejects a cache
file that changes while it is copied. The caller retains ownership of the
Datamonger cache, and `ArtifactStore::import_source` can persist another verified
copy without adopting that cache.

The acquisition receipt retains the exact registry selector, resolved dataset
ID and version, complete artifact inventory, representation and decoding
options, verification level, canonical digest, and Datamonger client version.
Its `external_dataset` evidence feeds the existing dataset derivation identity.
The selector, verified source tree, decoding recipe, verification claim,
materializer code and environment, canonical parameters, and dataset seed bind
the construction. Output hashes remain separate.

## Offline use

`offline = true` passes Datamonger's explicit offline option to every API call.
The pinned index must be bundled or cached, and every artifact must already be
cached and valid. A missing or corrupt cache entry fails without network access
or a different selection.

After source resolution, materialization and implementation execution require
only the verified local export. They can proceed after the registry server,
Datamonger interpreter, and provider cache become unavailable. Each implementation
receives a private view of the resulting standard dataset through the existing
artifact store. The example tests remove the provider before this stage and
check equal input hashes, equal dataset identities, distinct view paths, exact
results, and independent evaluator metrics.

## Compatibility and verification

`DATAMONGER_ADAPTER_VERSION = 1` versions the selector and standalone command.
The request passes one canonical JSON argument to the embedded Python bridge;
the bridge writes a response receipt outside the exported source tree. The
public `datamonger-selector`, `datamonger-acquisition-request`, and
`datamonger-acquisition-response` schemas declare JSON Schema 2020-12 and initial
compatibility version 1. Their positive and negative vectors live beside the
schemas; canonical standalone vectors and the test registry live in
[`fixtures/datamonger/v1`](../fixtures/datamonger/v1).

These entry points preserve the version-1 manifest, source-resolution, identity,
tree-hash, wire-protocol, and public-schema contracts. Execution semantics
version 2 adds [bounded concurrency](scheduling.md) independently of acquisition.

The ordinary workspace tests exercise the process bridge with a public-API test
double and run the example's materialization and private-view comparison offline.
The optional actual-client test uses a local HTTP registry and checks wrong
registry pins, absent versions, artifact mismatches, decoded digest mismatches,
artifact-only claims, verified offline cache hits, offline cache misses, and
execution after the server and cache are removed:

```sh
METEWAND_DATAMONGER_PYTHON=/absolute/path/to/python \
  cargo test --locked -p metewand-runtime --example datamonger -- --ignored
```

The interpreter must have Datamonger and its dependencies installed. The test
contacts only its local registry server and requires no public dataset service.
