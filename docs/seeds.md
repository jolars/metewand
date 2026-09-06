# Version-1 seed derivation

Metewand derives independent dataset, implementation, and scheduling seeds from
each experiment's authored safe-integer seed. Every derivation returns a
`DerivedSeedRecord` containing both the nonnegative 53-bit value used by workers
and the complete SHA-256 digest retained for identity and provenance records.

## Transcript

For role `dataset`, `implementation`, or `scheduling`, the digest is:

```text
SHA-256("metewand-seed-v1" || NUL || UTF-8(role) || NUL || canonical_fields)
```

`canonical_fields` is RFC 8785 JSON in Metewand's restricted version-1 value
domain. It has no whitespace or trailing newline. The three field objects are:

| Role | Canonical fields |
| --- | --- |
| Dataset | `dataset_definition`, `dataset_parameters`, `experiment_seed` |
| Implementation | `dataset_configuration`, `experiment_seed`, `implementation_repetition`, `problem_configuration` |
| Scheduling | `benchmark_name`, `experiment_name`, `experiment_seed`, `scheduling_policy_version` |

Identity fields contain the complete typed identity string. Dataset parameters
are the resolved canonical object after defaults and validation. Integer fields
must remain within the safe JSON range. Object properties appear in canonical
order in the transcript, independent of their input order.

`SEED_DERIVATION_VERSION` is `1` and selects the
`metewand-seed-v1` domain. `SCHEDULING_POLICY_VERSION` is also `1` and is
included as a field of every current scheduling derivation. Changing either
contract requires an explicit compatibility review and new golden vectors.

## Exposed value

The worker-visible value is the first 53 bits of the derivation digest,
interpreted most-significant bit first. Equivalently, implementations read the
first eight digest bytes as a big-endian `u64` and shift right by 11. This
produces a nonnegative integer no greater than `2^53 - 1`, so every supported
language and JSON implementation can represent it exactly.

The remaining digest bits are not discarded from the record. The full digest
distinguishes derivations that happen to share the same exposed value and binds
the complete derivation into configuration and logical-specification
identities.

## Dependency boundaries

Dataset seeds change with the experiment seed, dataset-definition identity, or
resolved dataset parameters. Implementation seeds change with the experiment
seed, dataset-configuration identity, problem-configuration identity, or
zero-based implementation-repetition index. They deliberately omit the
implementation identity, which gives every implementation in the same problem
and replication block the same value.

Scheduling seeds bind the benchmark and experiment names and the scheduling
policy version. They omit the expanded member set, so adding an unrelated
candidate does not perturb the scheduling seed. Neither derivation depends on
matrix-expansion order.

## Rust API and conformance vectors

`metewand_core::seed` exposes one derivation function and one canonical-field
function for each role. The canonical-field functions make the exact hashed
bytes inspectable by conformance tools.

[`fixtures/seeds/v1.json`](../fixtures/seeds/v1.json) fixes the canonical field
bytes, complete digest, and exposed value for all three roles. Rust consumes
this file directly, and future R, Python, and Julia SDKs must use the same
vectors without translating their contents.
