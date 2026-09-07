# Worker protocol handshake

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
capability selected by the plan. Additional reported capabilities are retained
as metadata but remain inert. Gate 1 selects only `one_shot`; the later
applicability and profile subprotocols define the semantics of the other names.

## Validation and negotiation

The checked-in
[`worker-protocol-message` schema](../schemas/v1/worker-protocol-message.schema.json)
is the source of truth for the handshake shapes. It rejects unknown fields,
empty request IDs, unsupported roles or capability names, duplicate protocol
versions and capabilities, malformed resolved-worker identities, and any
response whose `ok` or `protocol` discriminator is not the version-1 value.

After schema-compatible decoding, the orchestrator verifies that:

1. the response ID equals the pending hello request ID;
2. protocol 1 was offered and selected;
3. the response repeats the resolved worker identity from the request; and
4. every capability selected by the plan was reported by the worker.

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

## Encoding boundary

`encode_message` emits the canonical JSON representation followed by exactly
one line-feed byte and rejects a complete line larger than 1 MiB.
`decode_message` converts a frame from the bounded
[`FrameReader`](worker-protocol-framing.md) into the message type expected by
session state. The byte-oriented reader remains responsible for invalid UTF-8,
byte-order marks, duplicate JSON keys, malformed JSON, early EOF, and the same
line-size limit.

The wire-protocol version and public-schema compatibility version both remain
1. This handshake completes part of the initial, unreleased contracts; it does
not revise an earlier released format. Work-method messages, process transport,
and execution exchanges remain separate roadmap slices.
