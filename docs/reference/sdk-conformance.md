# SDK conformance

The normal workspace tests run the same version-1 compatibility vectors in
Rust, Python, and R. Python uses only its standard library; R uses the SDK's
native canonical JSON implementation and `digest`. The development environment
and CI provide both interpreters and the R build dependencies.

## Shared vectors

| Contract | Checked-in vectors | Checks |
| --- | --- | --- |
| Canonical JSON | [`fixtures/canonical-json/v1.json`](https://github.com/jolars/metewand/blob/main/fixtures/canonical-json/v1.json) | Accepted values produce the exact UTF-8 bytes; duplicate keys, unsafe numbers, and invalid Unicode fail. |
| Identities | [`fixtures/identities/`](https://github.com/jolars/metewand/tree/main/fixtures/identities) | Canonical transcripts and typed SHA-256 identities agree, including diagnostic details, dataset derivations, and concurrency policies. |
| Seeds | [`fixtures/seeds/v1.json`](https://github.com/jolars/metewand/blob/main/fixtures/seeds/v1.json) | All three roles reconstruct the canonical fields, complete digest, and first 53 digest bits. |
| Errors | [`fixtures/worker-protocol/v1/errors.json`](https://github.com/jolars/metewand/blob/main/fixtures/worker-protocol/v1/errors.json) | All four failure codes preserve messages and optional object details in exact response frames; invalid failures are rejected. |
| Protocol | [`fixtures/worker-protocol/v1/golden.json`](https://github.com/jolars/metewand/blob/main/fixtures/worker-protocol/v1/golden.json) | Complete sessions for every role, request validation, nullable timing, scientific verdicts, and malformed frames agree. |

The SDK tests reconstruct identity and seed transcripts only for conformance.
Workers receive their identities and seeds from Metewand; the SDK APIs do not
derive seeds, seed scientific libraries, schedule work, or construct run
identities.

Protocol session fixtures contain exact newline-terminated request and response
strings. The harness replaces `@ROOT@` with a temporary output directory and
`@SDK@` with the SDK name. These are fixture substitutions, not wire fields.
Rust decodes and reencodes every typed message. Python and R dispatch the
sessions through their SDK servers and compare every response byte.

## Timing and completed output

Deterministic clocks fix the execute callback's diagnostic duration at 25 ns.
Ordered event assertions place preparation before the first clock read and
statistics validation and result writing after the second. R also stores the
returned state after the second clock read. Tests reopen the result manifest and
its declared files, check file hashes, and read metrics before a successful
response is written. This catches results that become visible only after the
reply.

`implementation_time_ns` measures only the user execute callback. Result
serialization remains inside the orchestrator's active execution envelope and
outside this SDK diagnostic. These tests use fake clocks so slow machines and
timer resolution cannot change their expected results.

## Reference problem contracts

[`fixtures/problem-contract/v1/suites/`](https://github.com/jolars/metewand/tree/main/fixtures/problem-contract/v1/suites)
contains two benchmark-owned contracts with complete schemas, datasets, result
artifacts, expected metrics, and Python and R workers:

| Suite | Correct behavior | Semantic rejection |
| --- | --- | --- |
| `sum` | Sum the supplied values and add the problem's offset; report the input count. Cases include mixed signs and an empty input. | Reject an incorrect input count even when the reported sum is exact. |
| `argmin` | Report a minimum value and its zero-based index, choosing the first occurrence. Cases include tied and unique minima. | Reject another index, including a later tied minimum with the same value. |

The runtime test parses each contract, validates its semantics and reference
artifacts, and runs both evaluators on the checked-in reference results. It then
runs both SDK implementations against both evaluators. Each implementation has
five variants: correct, missing a required field, returning a wrong field type,
numerically wrong, and semantically invalid.

Malformed outputs fail the Rust scientific-schema validator before evaluation.
Schema-valid numerical errors retain metrics and receive `not_reached`;
semantically invalid outputs receive `invalid` even when their numerical error
is zero. Only the correct variants receive `reached`. Exact expected results,
metrics, and verdicts are checked independently of transport success.

These suites exercise the current library interfaces. The `metewand check
--workers` command is a later authoring slice.

## Running the checks

Run all checks with `devenv test`, or run the focused conformance tests inside
`devenv shell`:

```sh
cargo test -p metewand-core --test canonical_json --test identity_contract \
  --test identities --test inspection --test dataset_derivation --test seeds --locked
cargo test -p metewand-protocol --test golden_vectors --locked
cargo test -p metewand-runtime --test python_sdk --test r_sdk \
  --test problem_contract_conformance --locked
```

The runtime tests run the Python unit suite and install R into temporary package
libraries before running its suite. Missing interpreters or dependencies fail
the checks rather than silently skipping a language.

## Compatibility review

This slice preserves manifest, public-schema, canonicalization, identity, seed,
scheduling, and wire-protocol version 1 and execution-semantics version 2. It
adds conformance fixtures and enforces the existing callback timing boundary.
The diagnostic timing refinement does not change trusted timing scopes,
scientific validity rules, identities, or wire shapes. No compatibility constant
changes are required.
