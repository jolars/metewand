# Datamonger adapter vectors

`selector.json`, `request.json`, and `response.json` contain exact canonical
version-1 JSON bytes without a trailing newline. Runtime tests validate them
against the public schemas and assert canonical byte equality. Their registry
digest is a synthetic pin for the public-API process test double in `client.py`.

`data.csv` is a generated, three-row integer table. Its raw SHA-256 is
`398691f9ae1cc7dd7f4b7807369311a057ce75d7ef55442350ab7fe46d69edbb`.
Its `value` int64 vector has Datamonger canonical-form-1 SHA-256
`69abd67c3fa8b7ea2992b7a5091adf6626066bd2cb1e3e2e92222de8cc0fedfa`,
computed with the reference client. The fixture contains no third-party data.

`index.json` describes that table using Datamonger's revision-1 registry format.
The actual-client test replaces `127.0.0.1:PORT` with its private HTTP server and
pins the resulting exact index bytes. It also serves independently pinned
variants with bad artifact and decoded digests. These variants establish that
the actual provider rejects verification failures, while artifact-only mode
does not claim decoded verification. The test stops the server before offline
cache resolution, then removes the provider cache before materialization and
the two implementation measurements.
