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
Global option parsing stops at `--`, allowing authoring destinations such as
`metewand --output json init -- --output`.

[`init`](authoring-cli.md) emits one terminal result with `command`, `dry_run`,
`manifest`, and an ordered `paths` list. Each entry identifies a created or
proposed `path` and its `kind` (`directory` or `file`). The `machine-output`
schema defines this command's result shape.

[`scaffold`](authoring-cli.md#component-scaffolding) emits one terminal result
with `command`, `component`, `name`, `dry_run`, `manifest`, and ordered `paths`.
Each path has `path`, `kind` (`directory` or `file`), and `action` (`create` or
`modify`). The manifest is the only modified path and is reported last. The
`machine-output` schema defines this additive version-1 result shape.

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
line. Read-only commands produce one terminal `result` or `diagnostic` record.
[`run`](run-cli.md) streams ordered `event` records before its terminal record
in JSON Lines mode; its JSON result includes the same history in `data.events`.
The `run-output` schema defines those events and final reports. Consumers must
select behavior from `schema_version`, not from the package version.

[`status` and `explain`](inspection-cli.md) emit one terminal result conforming
to `inspection-output`. Status includes all recorded attempts, so a successful
inspection exits with `0` even when the inspected benchmark has failures.

[`export`](exports.md) emits one terminal `export-output` receipt with record
and row counts, the explicitly created destination or rebuilt index path, and
the export manifest. `--format jsonl|csv` selects the table file's encoding;
the global `--output` option continues to select the receipt's encoding.

The command-owned `data` object retains deterministic ordering. A plan result
contains the same definitions, resource identities, requirements, experiments,
candidate and slot identities, capability evidence, counts, and side-effect
claims shown by the human renderer. A schema-document result embeds the parsed
schema under `data.document`; omit `--output` when exact checked-in schema bytes
are required.

A successful [`check --workers`](worker-checks.md) result additionally includes
`workers` with `verified_now` capability evidence and `reference_cases` with
matching canonical metrics. Plain `check` omits these fields. The command
produces one terminal envelope in either machine format and keeps worker logs
out of standard output.

## Standard streams

Successful human output is written to standard output. Human diagnostics are
written to standard error.

In `json` and `jsonl` modes, standard output contains only schema-conforming
machine envelopes. A failed command writes its diagnostic envelope to standard
output and renders that same typed diagnostic for people on standard error.
Progress and other human-oriented messages also belong on standard error. A
consumer can therefore parse standard output without filtering prose.

Execution failures represented by finalized attempts produce a terminal run
result with failure counts and exit code `5`. Their typed diagnostics appear
inside the attempt summaries and are rendered on standard error. A failure
outside an attempt, such as preflight or publication, produces a diagnostic
envelope.

## Exit codes

Exit codes are part of the command-line compatibility contract:

| Code | Meaning |
| ---: | --- |
| `0` | The command completed successfully. A closed downstream pipe is also treated as successful cancellation. |
| `2` | The command line is invalid, or the requested operation is not supported. |
| `3` | The repository or logical plan is invalid. |
| `4` | A required manifest or repository input is unavailable. |
| `5` | Execution completed with at least one durably failed attempt. |
| `70` | An internal invariant or serialization operation failed. |
| `74` | Filesystem authoring, command output, run-directory setup, or durable publication failed. |

Export uses `2` for malformed options or unsafe index targets, `4` for missing
store inputs or unknown result selectors, `3` for incompatible or corrupt
evidence, and `74` for snapshot contention or export writes. It reuses the
inspection diagnostic codes for source-reading failures. Its own codes are
`missing_export_value`, `duplicate_export_option`, `invalid_export_id`,
`missing_output_directory`, `missing_export_destination`,
`invalid_export_format`, `export_id_not_found`, `export_write_failed`,
`invalid_export_destination`, and `invalid_export_evidence`.

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

The CLI emits these stable codes:

| Code | Meaning |
| --- | --- |
| `missing_output_format` | `--output` has no value. |
| `invalid_output_format` | The output-format value is invalid. |
| `duplicate_output_format` | `--output` was supplied more than once. |
| `invalid_command_name` | The command name is not UTF-8. |
| `unknown_command` | The command name is not recognized. |
| `invalid_init_path` | The initialization destination is empty or not UTF-8. |
| `duplicate_init_flag` | `--dry-run` was supplied more than once. |
| `init_path_exists` | A manifest path or required directory conflicts with an existing entry. |
| `init_io_failed` | Initialization could not validate or create a path. |
| `invalid_scaffold_component` | The scaffold component is missing or is not dataset, problem, or implementation. |
| `invalid_scaffold_argument` | A scaffold name or path is empty or is not UTF-8. |
| `missing_scaffold_argument` | A required scaffold name, environment, or option value is missing. |
| `duplicate_scaffold_flag` | A scaffold option was supplied more than once. |
| `scaffold_invalid_request` | A component name is invalid, an environment or problem is undefined, or the problem selection does not match the component. |
| `scaffold_component_exists` | The requested component name is already defined in its manifest namespace. |
| `scaffold_invalid_manifest` | The existing manifest could not be parsed or the component addition could not be represented. |
| `scaffold_path_exists` | A generated path conflicts with an existing entry, or a required directory or manifest is a symbolic link or has the wrong type. |
| `scaffold_manifest_busy` | Another operation holds the manifest lock. |
| `scaffold_manifest_changed` | The manifest file or contents changed before the requested addition could be written. |
| `scaffold_io_failed` | Scaffolding could not read, create, or modify a path. |
| `invalid_schema_name` | A schema name is not UTF-8. |
| `unknown_schema` | A public schema name is not recognized. |
| `unexpected_argument` | A command or option received an extra argument. |
| `empty_manifest_path` | The selected manifest path is empty. |
| `missing_manifest_path` | `--manifest` has no path argument. |
| `duplicate_manifest_path` | `--manifest` was supplied more than once. |
| `missing_output_directory` | `--output-dir` has no path, or `run` lacks its required destination. |
| `empty_output_directory` | The selected run-directory path is empty. |
| `duplicate_output_directory` | `--output-dir` was supplied more than once. |
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
| `invalid_run_selection` | A selected ID matches no current measured slot, or a resume destination is invalid. |
| `invalid_max_attempts` | The attempt bound is not a positive safe integer. |
| `duplicate_max_attempts` | The attempt bound was supplied more than once. |
| `missing_run_selection` | A selection or attempt bound lacks a value. |
| `duplicate_run_flag` | A resume or retry flag was supplied more than once. |
| `run_store_failed` | Durable run coordination or recovery failed. |
| `missing_inspection_id` | `explain` lacks its required stable identity. |
| `invalid_inspection_id` | The explanation target is not a complete supported identity. |
| `inspection_id_not_found` | The exact ID is absent from the selected plan and store. |
| `inspection_input_unavailable` | A required run-store or artifact input could not be read. |
| `inspection_store_busy` | A changing store prevented a stable private snapshot within five seconds. |
| `invalid_inspection_evidence` | The store schema, committed records, or artifact contents are invalid or incompatible. |
| `coordination_failed` | The nested run-store diagnostic explains the failure. |
| `run_setup_failed` | Execution resolution or destination setup failed. |
| `execution_input_unavailable` | A required executable or run path is unavailable. |
| `execution_resolution_failed` | An internal resolution invariant failed. |
| `invalid_execution_input` | Fixed data or other resolved inputs are invalid or changed. |
| `artifact_cache_failed` | Artifact publication, integrity checking, construction lookup, or a cache lease failed. |
| `run_publication_failed` | An execution artifact could not be finalized. |
| `publication_failed` | The nested publisher diagnostic explains why publication failed. |

The run result also retains attempt diagnostic codes: `setup_failure`,
`implementation_failure`, `implementation_crash`, `scientific_timeout`, `phase_timeout`,
`invalid_result`, `evaluator_failure`, `protocol_error`, `input_changed`, and
`completion_target_not_reached`. See [execution output](run-cli.md) for their
publication and exit behavior.

Adding a new failure mode may add a diagnostic code. Changing the meaning of an
existing code or exit code requires an explicit compatibility review.
