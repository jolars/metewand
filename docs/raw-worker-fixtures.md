# Raw worker fixtures

Metewand includes executable version-1 workers under
[`fixtures/worker-protocol/v1`](../fixtures/worker-protocol/v1). They implement
the protocol directly with the Python standard library and report `sdk: null`,
so tests exercise the process and wire boundaries without depending on a
language SDK. The development environment supplies Python 3 for reproducible
local use.

## Successful roles

The `dataset-materializer`, `implementation`, and `problem-evaluator`
entrypoints provide a small deterministic computation that can be reused by
runtime and end-to-end conformance tests.

The materializer accepts an integer `values` array in the dataset parameters.
It writes `data.json`, computes its exact SHA-256 digest and size, and writes
the complete `dataset-manifest.json` last. The implementation's `prepare`
request reads that dataset and requires `algorithm = "sum"`; `execute` adds the
integer problem parameter `offset` to the values' sum and writes the canonical
answer to `result.json`. `reset` clears the prepared state. The independent
evaluator reads the dataset and result, recomputes the expected answer, and
writes `absolute_error` to its assigned metrics path.

The adjacent schemas validate the materialized dataset manifest, canonical
result, and evaluator metrics. All artifacts and protocol responses are
deterministic for the same requests.

## Protocol abuse

Invoke `protocol-failure` with one mode to produce a specific adversarial
response after it reads the orchestrator's hello request:

| Mode | Behavior or typed failure exercised |
|---|---|
| `fragmented_reads` | Sends valid hello and shutdown responses one byte at a time. |
| `duplicate_keys` | Repeats the top-level `id` key. |
| `invalid_utf8` | Places an invalid byte in the response. |
| `byte_order_mark` | Prefixes the response with a UTF-8 byte-order mark. |
| `oversized_line` | Sends a line beyond the version-1 1 MiB limit. |
| `malformed_json` | Sends incomplete JSON syntax followed by a newline. |
| `early_eof` | Closes the response descriptor after an unterminated partial line. |
| `extra_response` | Sends two valid responses for the hello request. |
| `mismatched_request_id` | Answers hello with a different request ID. |

The runtime integration tests launch the entrypoints through the dedicated
POSIX protocol descriptors. They validate the successful three-role artifact
chain and assert the exact framing or correlation error produced by every
adversarial mode. Standard output and standard error remain available only for
worker logs.
