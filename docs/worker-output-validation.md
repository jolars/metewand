# Worker output validation

A successful worker exchange reports completion; it does not make the worker's
paths or files trustworthy. `metewand-runtime` validates materializer,
implementation, and evaluator output in the private directory assigned by the
executor before any artifact can be accepted or published.

`WorkerOutputValidator` takes the offline schema catalog from a checked
repository. It also loads Metewand's embedded public schemas, so validation does
not read schemas from the worker's output or retrieve them from the network.

## Paths and filesystem inventory

Materializer and implementation responses name their manifest relative to the
assigned output root. Returned paths must be nonempty, normalized UTF-8 relative
paths using `/` separators. Absolute POSIX and Windows drive paths, `.` and `..`
components, repeated or trailing separators, and backslashes are rejected
before the path is joined to the root.

The runtime walks the output without following links. Regular files and the
directories needed to contain them are the only accepted entry types. Symbolic
links, sockets, FIFOs, devices, and other special entries fail validation. The
same walk rejects files absent from the manifest and directories that contain
no declared file. A result or dataset manifest cannot declare itself because a
self-hash cannot be completed.

Each declared payload path must be unique. The runtime checks that it names a
regular file, compares its exact byte length, streams it through SHA-256, and
detects length or type changes during the read. Validated file records are
returned in UTF-8 byte order, independent of their order in the worker's JSON.

Evaluator output uses the same containment rules. Its assigned metrics document
must be the only regular file in its private output; only directories needed to
contain that document are permitted.

## Document and scientific contracts

Every document is parsed through Metewand's restricted canonical JSON domain,
which rejects malformed input, duplicate keys, invalid Unicode, non-finite or
out-of-range numbers, and trailing input. Materialized datasets must satisfy the
public `artifact-manifest` schema—including its `complete: true` marker—and the
dataset definition's output schema.

Implementation results must satisfy the public `result-manifest` schema. The
manifest's `schema` field must exactly name the result schema selected by the
checked problem contract, and its `data` object is validated against that
schema. Evaluator metrics follow the analogous rule for the public `metrics`
envelope and the selected metric schema. A worker cannot redirect validation by
naming a different repository schema in its output.

The validator returns typed `ValidatedDataset`, `ValidatedResult`, and
`ValidatedMetrics` values. Failures retain the rejected relative path, selected
schema, expected and observed size or digest, or underlying JSON and schema
diagnostics as appropriate. Artifact publication and attempt-outcome mapping
remain later runtime layers; neither protocol success nor an unvalidated path
is evidence of a valid result.
