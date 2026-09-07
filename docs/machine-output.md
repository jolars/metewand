# Machine-readable command output

Every Metewand command supports the global `--output` option. The option may
appear before or after the command:

```sh
metewand --output json check
metewand plan --output jsonl
metewand schema manifest --output json
```

The accepted formats are `human`, `json`, and `jsonl`; `human` is the default.
The option may be supplied at most once.

## Versioned envelopes

Both machine formats use the published `machine-output` schema. Every emitted
record has `schema_version`, `kind`, and `data` fields:

```json
{
  "schema_version": 1,
  "kind": "result",
  "data": {
    "command": "check",
    "benchmark": "example"
  }
}
```

`json` writes one indented envelope. `jsonl` writes one compact envelope per
line. The current read-only commands produce one terminal `result` or
`diagnostic` record; later commands may precede the terminal record with
`event` records. Consumers must select behavior from `schema_version`, not from
the package version.

The command-owned `data` object retains deterministic ordering. A plan result
contains the same definitions, resource identities, requirements, experiments,
candidate and slot identities, capability evidence, counts, and side-effect
claims shown by the human renderer. A schema-document result embeds the parsed
schema under `data.document`; omit `--output` when exact checked-in schema bytes
are required.

## Standard streams

Successful human output is written to standard output. Human diagnostics are
written to standard error.

In `json` and `jsonl` modes, standard output contains only schema-conforming
machine envelopes. A failed command writes its diagnostic envelope to standard
output and renders that same typed diagnostic for people on standard error.
Progress and other human-oriented messages also belong on standard error. A
consumer can therefore parse standard output without filtering prose.

## Exit codes

Exit codes are part of the command-line compatibility contract:

| Code | Meaning |
| ---: | --- |
| `0` | The command completed successfully. A closed downstream pipe is also treated as successful cancellation. |
| `2` | The command line is invalid, or the requested operation is not supported. |
| `3` | The repository or logical plan is invalid. |
| `4` | A required manifest or repository input is unavailable. |
| `70` | An internal invariant or serialization operation failed. |
| `74` | Command output could not be written. |

The exit code classifies the command outcome; `data.code` identifies the
specific diagnostic.

## Structured diagnostics

A diagnostic always contains a stable `code` and a human-readable `message`.
It may also contain:

- `details`, for code-specific structured context;
- `source_span`, with one-indexed lines and columns and an exclusive end
  position;
- `affected_ids`, as a sorted, duplicate-free list of stable identities;
- `causes`, as ordered nested diagnostics; and
- `remediation`, when Metewand can state a safe concrete correction.

Human diagnostics are rendered from this record. Source locations, causal
indentation, affected identities, and remediation therefore cannot diverge
between the human and machine paths. Fields that do not apply are omitted. In
particular, validation failures that occur before identity construction have no
`affected_ids` field.

### Diagnostic codes

The read-only CLI emits these stable codes:

| Code | Meaning |
| --- | --- |
| `missing_output_format` | `--output` has no value. |
| `invalid_output_format` | The output-format value is invalid. |
| `duplicate_output_format` | `--output` was supplied more than once. |
| `invalid_command_name` | The command name is not UTF-8. |
| `unknown_command` | The command name is not recognized. |
| `invalid_schema_name` | A schema name is not UTF-8. |
| `unknown_schema` | A public schema name is not recognized. |
| `unexpected_argument` | A command or option received an extra argument. |
| `empty_manifest_path` | The selected manifest path is empty. |
| `missing_manifest_path` | `--manifest` has no path argument. |
| `unsupported_operation` | The requested operation is deliberately unavailable. |
| `repository_check_failed` | The repository-validation operation failed; `causes` identifies why. |
| `manifest_access_failed` | The manifest could not be accessed. |
| `manifest_not_file` | The manifest path is not a regular file. |
| `repository_file_access_failed` | A declared repository file could not be read. |
| `repository_path_escape` | A declared path resolves outside the repository. |
| `invalid_utf8_file` | A declared text file is not UTF-8. |
| `invalid_schema_json` | A repository schema is not valid Metewand JSON. |
| `invalid_schema_reference` | A schema reference is not a valid repository reference. |
| `missing_schema_identity` | A loaded schema unexpectedly lacks a content identity. |
| `invalid_typed_digest` | A validated digest could not be decoded. |
| `invalid_manifest` | The manifest does not satisfy the typed format. |
| `invalid_manifest_reference` | A typed manifest reference cannot be resolved. |
| `invalid_problem_contract` | A problem contract cannot be parsed or validated. |
| `invalid_schema_catalog` | The offline repository schema catalog is invalid. |
| `invalid_configuration` | Parameter expansion or cross-definition compatibility failed. |
| `invalid_source_bundle` | A declared source bundle cannot be expanded or identified. |
| `canonicalization_failed` | A value cannot enter the canonical JSON domain. |
| `identity_construction_failed` | A stable identity cannot be constructed. |
| `logical_planning_failed` | Logical planning failed; `causes` identifies why. |
| `missing_definition_identity` | Planning lacks an identity for a selected definition. |
| `seed_derivation_failed` | A deterministic seed cannot be derived. |
| `internal_error` | An internal invariant or output serialization failed. |
| `output_write_failed` | Standard output could not be written. |

Adding a new failure mode may add a diagnostic code. Changing the meaning of an
existing code or exit code requires an explicit compatibility review.
