# Worker protocol and execution exchanges

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

The protocol recognizes `one_shot`, `applicability`, and `buffered_profile`.
These capabilities belong to implementation workers. A session rejects a worker
missing a selected capability. `fresh_sequence` supports selected controlled execution; `streaming_profile`
remains recognized metadata whose selection is reserved. Unselected reports are inert.

Evaluators may report `problem_semantics`. Metewand selects it when reported
and sends the complete canonical scientific rules in `evaluate` requests. It
cannot be selected for implementation or materializer workers.

## Validation and negotiation

The checked-in
[`worker-protocol-message` schema](https://github.com/jolars/metewand/blob/main/schemas/v1/worker-protocol-message.schema.json)
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
5. capabilities match the worker role, including evaluator-only `problem_semantics`; and
6. no reserved profile capability was selected.

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

When the evaluator negotiated `problem_semantics`, the request also contains
`"problem_semantics": { ... }`, holding the entire contract `semantics` object.
This includes preprocessing, validity, completion, and other family-owned rules.
The field must be an object and is required in sessions that select this
capability. It is omitted for legacy evaluators. Run execution and reference
checks deliver the same rules. Evaluator code should read declared tolerances
from this object.

Evaluation returns a required problem-owned verdict after writing metrics:

```json
{"id":"1","ok":true,"verdict":"reached"}
```

The verdict is `invalid`, `reached`, `not_reached`, or `not_required`; transport
success does not imply scientific validity. `shutdown` is the final request
for every role; the shutdown phase includes both its acknowledgment and clean
process exit. The [Python SDK](python-sdk.md) supplies callback helpers for all
three roles.

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

`WorkerSession` also accepts an absolute scientific deadline through
`connect_with_deadline` or `set_scientific_deadline`. Exchanges use the earlier
of that shared deadline and the current operational phase limit. Expiry returns
`ScientificTimeout` and discards the worker. The orchestrator can clear the
scientific deadline before untimed phases without changing their operational
limits. The fixed-dataset runtime and CLI use this mechanism for all three
[timing envelopes](fixed-dataset-run.md).

## Optional applicability exchange

An implementation session that selects `applicability` may send
`check_applicability` before measured preparation:

```json
{"id":"1","method":"check_applicability","dataset_dir":"/private/data","dataset_manifest":"dataset-manifest.json","problem_contract_id":"contract-id","problem_parameters":{"intercept":true},"implementation_parameters":{}}
```

The successful response distinguishes intrinsic support from a failed worker
operation. Both outcomes require an implementation-owned stable reason code
and a human-readable reason. Optional `details` must be an object:

```json
{"id":"1","ok":true,"outcome":"not_applicable","code":"unsupported_intercept","reason":"This solver requires a zero intercept.","details":{"intercept":true}}
```

The other successful outcome is `applicable`. `unchecked` describes the absence
of a check and is never a worker response. A worker that cannot determine support
returns the usual `ok: false` failure envelope.

`WorkerSession::check_applicability` returns both the exact request and the
response so callers can retain the checked inputs. It requires the implementation
role and selected capability and uses the preparation operational timeout.
Callers must clear the scientific deadline and provide a private immutable
dataset view before this untimed exchange. The manifest path may be absolute
or relative to that view.

Native workers can opt in through `worker_sdk::Implementation` by returning
`true` from `supports_applicability` and implementing `check_applicability`.
The default remains unsupported, so existing native workers keep their current
handshake and behavior. Python workers opt in by overriding `check_applicability(dataset_instance,
dataset_manifest, problem_contract, problem_parameters, implementation_parameters)`
and returning a dictionary with `outcome`, `code`, `reason`, and optional
`details`. `dataset_instance` and `dataset_manifest` are `Path` objects.

Declare `capabilities: [one_shot, applicability]` in the manifest. `plan` then
records `unchecked` with code `check_pending` without launching workers. During
`run`, Metewand materializes the dataset and checks each logical candidate in a
separate worker before allocating measured slots. Checks run outside all
performance timing scopes, including cold end-to-end timing. An exclusion is a
successful decision and creates no resolved runs, attempt slots, or observation
slots. It runs no warm-ups, preparation, execution, or evaluation. A failed
callback or invalid response aborts resolution with an error.

Persisted decisions retain the exact problem and implementation parameters,
problem contract identity, dataset instance identity, dataset manifest content
and relative path, worker and environment identities, observed local controls,
and structured response. Temporary view and worker paths are normalized in
evidence so repeated resolution preserves identities. For the built-in empty
unit dataset, the manifest path in the record is null, and the callback receives
a separate synthesized manifest. The private dataset view contains independent
copies, so writes through that view cannot change the shared dataset. The runner
verifies the view, its manifest, and the canonical dataset after shutdown,
including failed checks and exclusions. A changed view aborts resolution before
the runner allocates slots or initializes a new run store.

`explain <decision-id> --output-dir <run>` exposes this evidence, and `export`
retains it for both applicable and excluded candidates. Implementations without
the capability keep unconditional support, recorded as `applicable` with code
`unconditional_one_shot`, without a worker check.

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
validation and profile subprotocols remain separate roadmap slices.

## Buffered execution

`execute_buffered` requires the negotiated `buffered_profile` capability and a
successful preparation. Its request contains `id`, `method`, `result_root`, and
`profile`, the normalized selected policy and instrumentation. The successful
response contains `id`, `ok: true`, and `buffer: {manifest: "buffer.json"}`.
There are no checkpoint or continuation messages.

All trace and snapshot files must be closed before the response. The buffer
manifest binds the file inventory, local stop reason, monotonic clock, scope,
trace, selected results, and total instrumented duration. `resource_limit` is a
worker failure code for output that cannot fit declared collection limits.
See [buffered profiles](buffered-profiles.md) for scientific and timing rules.

## Fresh-sequence execution

A selected `fresh_sequence` implementation receives the canonical
`observation_control` object in `execute`. Its response echoes that exact value;
Metewand rejects missing or mismatched echoes and unsolicited one-shot echoes.
Result manifests, preparation, reset, and shutdown retain their existing rules.
Python and R workers explicitly opt in through `serve_implementation`'s
`fresh_sequence` argument. See [Fresh-sequence profiles](fresh-sequence-profiles.md).
