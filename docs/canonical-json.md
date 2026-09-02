# Canonical JSON

Metewand version 1 uses a restricted JSON value domain wherever data must agree
across languages. Accepted values have one canonical UTF-8 representation. That
representation is the only JSON encoding used for content hashing and transport.

## Value domain

The version-1 domain contains JSON nulls, booleans, arrays, and objects, subject
to these restrictions:

- Every object key is unique after JSON escapes have been decoded. For example,
  `"a"` and `"\u0061"` are duplicate keys in the same object.
- Strings contain Unicode scalar values. Metewand rejects invalid UTF-8 and lone
  surrogates and does not apply Unicode normalization.
- Integral values lie in the inclusive range
  `[-9_007_199_254_740_991, 9_007_199_254_740_991]`.
- Other numbers are finite IEEE-754 binary64 values. JSON spellings for `NaN`
  and infinity are invalid, as are values that overflow binary64.

The integer rule applies to the parsed numeric value rather than its source
spelling. Thus `1e3` is the safe integer `1000`, while
`9.007199254740992e15` is outside the domain. Metewand normalizes safe integral
float spellings to integers and every spelling of negative zero to integer zero.

## Canonical bytes

Metewand serializes accepted values according to
[RFC 8785](https://www.rfc-editor.org/rfc/rfc8785.html). Canonical output:

- contains no insignificant whitespace or trailing newline;
- preserves array order and sorts object properties recursively by their UTF-16
  code units;
- uses ECMAScript-compatible binary64 number formatting; and
- emits the RFC-defined string escapes and otherwise preserves Unicode scalar
  values as UTF-8.

In Rust, `metewand-core` exposes the contract through `CanonicalValue`:

```rust
use metewand_core::canonical::CanonicalValue;

let value = CanonicalValue::from_slice(br#"{"b": 1e3, "a": -0.0}"#)?;

// Schema validation will inspect this normalized value before bytes are used.
let normalized = value.as_json();
assert!(normalized.is_object());

let bytes = value.to_canonical_bytes()?;
assert_eq!(bytes, br#"{"a":0,"b":1000}"#);

# Ok::<(), metewand_core::canonical::CanonicalJsonError>(())
```

`CanonicalValue` deliberately does not implement `Display` or `Serialize`.
Callers must request canonical bytes explicitly rather than assuming an ordinary
JSON serializer produces Metewand's wire or hashing representation.

## Conformance vectors

[`fixtures/canonical-json/v1.json`](../fixtures/canonical-json/v1.json) is the
shared cross-language contract. Its `accepted` entries pair source JSON with the
exact expected bytes, while its `rejected` entries identify values that every
implementation must refuse. Rust integration tests consume this file directly;
future SDKs must run the same vectors without translating their contents.

Schema validation is a separate step. A caller first parses the restricted
value, validates the normalized JSON against the selected schema, and only then
uses its canonical bytes for hashing or transport.
