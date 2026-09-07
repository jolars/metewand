# Worker protocol framing

Metewand's version-1 worker protocol carries one JSON value per line. The
`metewand-protocol` crate exposes `framing::FrameReader` as the transport-neutral
byte boundary: runtime transports can place it over a pipe or another byte
stream without assuming how reads are fragmented.

Each line must satisfy these framing rules:

- its bytes are UTF-8 and do not begin with a UTF-8 byte-order mark;
- it contains one complete JSON value with no duplicate object keys at any
  nesting depth;
- it ends with one line-feed byte; and
- its total size, including that line feed, is at most 1 MiB (1,048,576 bytes).

The limit is inclusive. A valid JSON payload of 1,048,575 bytes followed by its
line feed is accepted. The reader stops accumulating an unterminated line as
soon as no legal line feed can fit, which bounds memory use even when a worker
sends an unending stream of bytes.

Clean EOF between messages is distinct from early EOF within a message. The
former ends a stream; the latter is a typed `EarlyEof` failure. Byte-order marks,
invalid UTF-8, duplicate keys, malformed JSON, oversized lines, and underlying
I/O failures likewise have separate `FrameError` variants.

Framing accepts a general JSON value. Versioned message schemas and typed
request and response models impose the object shape, allowed fields, and
method-specific semantics at the next protocol layer. The version-1
[`hello` handshake](worker-protocol.md) is the first such typed layer.

## Response correlation

`framing::ResponseCorrelator` binds worker responses to the request currently
awaiting a response. A response must contain a string `id` equal to the pending
request ID. A different ID is a typed mismatch and leaves the expected request
pending; a second response after the expected one is accepted is a typed extra
response. The mismatch and extra-response checks operate on frames produced by
the same bounded reader.

The adversarial integration suite covers arbitrary fragmented reads, nested
duplicate keys, invalid UTF-8, byte-order marks, the inclusive size boundary,
oversized lines, malformed JSON, early EOF, extra responses, and mismatched
request IDs. These cases establish the byte-stream foundation independently of
the typed handshake and later worker execution exchanges.

The wire-protocol compatibility version remains 1. This work defines the
framing behavior of the initial, unreleased version rather than changing a
previously published contract.
