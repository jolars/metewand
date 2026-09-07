# Worker protocol and one-shot exchanges

Every worker session begins with a strict version-1 handshake before Metewand
sends work. The orchestrator's first JSON-Lines message fixes the requested
role and resolved worker identity while offering its supported protocol
versions:

```json
{"id":"0","method":"hello","protocols":[1],"role":"implementation","worker_id":"mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}
```

The worker selects version 1, reproduces the expected identity, and reports its
SDK and capabilities:

```json
{"capabilities":["one_shot"],"id":"0","ok":true,"protocol":1,"sdk":{"name":"metewand-python","version":"0.1.0"},"worker_id":"mw1-resolved-worker-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}
```

Direct protocol workers use `"sdk": null`. SDK-backed workers supply nonempty
package name and version strings. The three roles are `dataset_materializer`,
`implementation`, and `problem_evaluator`.

The protocol recognizes the `one_shot`, `applicability`, `fresh_sequence`, and
`streaming_profile` capability names. A session fails if the worker omits a
capability selected by the plan, and capabilities may be selected only for the
implementation role. Gate 1 selects only `one_shot`. Workers may report the
other three capabilities, and Metewand records those reports, but version-1
sessions reject attempts to select them until their request subprotocols are
implemented. An unselected report remains inert.

## Validation and negotiation

The checked-in
[`worker-protocol-message` schema](../schemas/v1/worker-protocol-message.schema.json)
is the source of truth for every message shape. It rejects unknown fields,
empty request IDs and paths, unsafe seeds and diagnostic times, unsupported
roles, capabilities, or failure codes, duplicate protocol versions and
capabilities, malformed resolved-worker identities, and invalid `ok` or
`protocol` discriminators.

After schema-compatible decoding, the orchestrator verifies that:

1. the response ID equals the pending hello request ID;
2. protocol 1 was offered and selected;
3. the response repeats the resolved worker identity from the request; and
4. every capability selected by the plan was reported by the worker;
5. capabilities were selected only for an implementation worker; and
6. no reserved applicability or profile capability was selected.

These checks produce typed `HandshakeError` values. The
`metewand-protocol` crate exposes the strict `HelloRequest` and `HelloResponse`
types, `negotiate_protocol`, and `validate_hello_response`. It also exports the
exact schema bytes and their stable path and identifier.

## Serial request rule

Every request and response carries a nonempty string ID. A
`RequestTracker` allocates session-local decimal IDs beginning with `"0"` and
allows one pending orchestrator request at a time. Beginning another request
before its response arrives is a typed error. A mismatched response does not
consume the pending request, and a response with no pending request is an extra
response error.

Version 1 has this one-request concurrency limit even when a worker process is
reused across logical attempts. The bounded checkpoint exchange added for
streaming profiles will remain nested inside its `execute_stream` request and
will not permit an unrelated concurrent request.

## Gate-1 one-shot methods

After the handshake, the negotiated role determines which requests the worker
may receive. A dataset materializer accepts `materialize`, an implementation
accepts `prepare`, `execute`, and `reset`, and a problem evaluator accepts
`evaluate`. Every role accepts `shutdown`. A wrong-role call fails locally and
does not consume a request ID.

Materialization receives the worker source directory, canonical dataset
parameters, a nonnegative 53-bit seed, and its assigned output directory:

```json
{"dataset_parameters":{"rows":100},"dataset_seed":17,"id":"1","method":"materialize","output_dir":"/private/dataset-output","source_dir":"/private/source"}
```

After closing every dataset file and its manifest, the worker identifies the
manifest beneath the assigned output:

```json
{"dataset":{"manifest":"dataset-manifest.json"},"id":"1","ok":true}
```

An implementation receives all attempt-specific state in `prepare`, followed
by a separate `execute` request. The one-shot request deliberately has no
scientific budget or observation control; those fields belong to later
capability subprotocols.

```json
{"dataset_dir":"/private/dataset","id":"1","implementation_parameters":{"solver":"fast"},"implementation_seed":23,"method":"prepare","problem_contract_id":"mw1-problem-contract-abc","problem_parameters":{"lambda":0.1}}
{"id":"2","method":"execute","result_dir":"/private/result"}
```

Successful `execute` responses identify the completed result manifest, carry
an object of implementation-owned diagnostic statistics, and explicitly use
`null` when no in-worker timing is available:

```json
{"id":"2","implementation_time_ns":18342011,"ok":true,"result":{"manifest":"result.json"},"statistics":{"iterations":37}}
```

`implementation_time_ns` is diagnostic provenance. Executor-observed time
remains authoritative. `reset` has only its ID and method. A successful
`prepare` or `reset` receives the common `{"id":"…","ok":true}`
acknowledgment.

The problem evaluator receives the private dataset and result directories,
the problem contract identity and canonical parameters, and the exact path at
which it must finish the metrics document:

```json
{"dataset_dir":"/private/dataset","id":"1","method":"evaluate","metrics_path":"/private/metrics.json","problem_contract_id":"mw1-problem-contract-abc","problem_parameters":{"lambda":0.1},"result_dir":"/private/result"}
```

Evaluation also receives the common acknowledgment. `shutdown` is the final
request for every role; the shutdown phase includes both its acknowledgment
and clean process exit.

The manifest paths in successful responses are worker-supplied data. The
[worker-output validation layer](worker-output-validation.md) resolves them
beneath the executor-assigned directory, validates their schemas and complete
file inventories, and checks every declared file before publication; an
exchange success alone never validates an artifact.

## Failures and deadlines

Any operation may return a structured failure instead of its success shape:

```json
{"error":{"code":"operation_failed","details":{"iterations":100},"message":"The implementation did not converge."},"id":"2","ok":false}
```

Version 1 defines four worker-owned codes: `invalid_request`,
`unsupported_operation`, `operation_failed`, and `internal_error`. The message
is nonempty, and optional details are an object. The runtime retains the
current phase with this payload so materialization, setup, execution, reset,
evaluation, and shutdown failures remain distinguishable.

`PhaseTimeouts` supplies a resolved duration for `hello`, `materialize`,
`prepare`, `execute`, `reset`, `evaluate`, and `shutdown`; the runtime has no
implicit fallback that could leave a phase unbounded. Requests and blocking
response reads run on a dedicated protocol-I/O thread, so the orchestrator can
enforce each deadline while continuing to drain worker logs. A timeout,
framing violation, malformed response, closed protocol stream, or failed reset
marks the session unusable and discards the process. Worker-reported failures
remain typed; failures other than reset do not by themselves claim that the
protocol session is corrupt.

## Encoding boundary

`encode_message` emits the canonical JSON representation followed by exactly
one line-feed byte and rejects a complete line larger than 1 MiB.
`decode_message` converts a frame from the bounded
[`FrameReader`](worker-protocol-framing.md) into the message type expected by
session state. The byte-oriented reader remains responsible for invalid UTF-8,
byte-order marks, duplicate JSON keys, malformed JSON, early EOF, and the same
line-size limit.

The wire-protocol version and public-schema compatibility version both remain
1. These one-shot messages complete another part of the initial, unreleased
contracts; they do not revise an earlier released format. Returned artifact
validation and the applicability and profile subprotocols remain separate
roadmap slices.
