# Result indexes and tidy exports

Export a recorded run to a new directory:

```sh
metewand export --output-dir runs/example --format csv --destination tables/example
metewand export --output-dir runs/example --destination tables/example-jsonl
```

The destination's parent must exist. `--destination` authorizes creating the
directory and its two files: `results.csv` or `results.jsonl`, and the adjacent
lineage manifest `export.json`. JSONL is the default. Metewand prepares both
files privately and publishes the directory without replacing an existing
path, including an empty directory or a symbolic link. These rebuildable
outputs do not carry the run store's durable-publication guarantee.

The global `--output human|json|jsonl` option controls the command receipt, not
the table encoding. Machine receipts conform to `export-output` inside the
existing `machine-output` envelope. A successful export exits with `0` even
when the selected history contains failed attempts.

## Authoritative evidence and the index

```sh
metewand export --output-dir runs/example --rebuild-index
```

`--rebuild-index` explicitly authorizes replacing only `index.jsonl` at the top
of the run directory. It can accompany `--destination`. The index always
contains the complete run, regardless of export filters. An existing index
must be a regular file; symbolic links and directories are rejected.

Each rebuild reads a private, committed SQLite snapshot and verifies registered
artifact inventories and content hashes through the same reader used by
[`status` and `explain`](inspection-cli.md). The reader does not initialize,
recover, or checkpoint the source store, launch workers, or require the
original repository. Unregistered directories never establish an observation
or acceptance. An unavailable store or corrupted artifact fails the command.

The index is a disposable projection, never coordination state. Exports always
read the database and finalized artifacts directly, even when an index is
present. A missing, stale, or malformed index cannot hide history, establish
acceptance, or authorize deletion. Rebuilding atomically replaces the index;
unchanged evidence produces identical canonical JSONL bytes. Neither index
creation nor export is an implicit side effect of inspection commands.

## Records and rows

The index contains one versioned `export-record` per committed applicability
decision, attempt, and observation. Both applicable and not-applicable decisions
retain their original evidence. Current one-shot runs register unconditional
applicability. Pending logical slots without allocated attempts are available
through `status`, rather than manufactured as attempts in the index.

Attempts retain every retry, including accepted, invalid, censored, superseded,
failed, and nonterminal attempts. `attempt_state` preserves the stored state.
A failed attempt with `failure_code: "invalid_result"` additionally has
`outcome: "invalid"` and `valid: false`. Other failed attempts have unknown
validity unless they produced a separately registered observation. Censoring
does not become a technical failure, and supersession does not invalidate an
earlier result.

Observations retain independent `valid`, `selected`, and `completion` fields,
as well as their producing attempt's current state and outcome. A valid,
unselected observation remains visible after its attempt fails, is censored,
or is superseded. No export averages repetitions, chooses a winning retry,
filters warm-ups, or drops attempts that lack metrics.

Tidy tables use `export-row`: one row per evaluator metric coordinate on an
observation, plus one row with a null metric coordinate for every entity
without metrics. Metric coordinates are RFC 6901 JSON pointers. Object keys
sort bytewise; arrays and empty nested objects remain intact JSON values rather
than becoming invented repetitions. An actual null metric value remains
distinguishable from an absent metric through its nonnull `metric_name`.

Both table formats expose numeric `timed_wall_time_ns`,
`implementation_time_ns`, `evaluation_time_ns`, `cpu_time_ns`, and
`peak_rss_bytes` columns. Wall time, CPU time, and resident-set size also carry
`complete`, `partial`, or `unavailable` status columns. Partial resource values
remain diagnostic evidence. The original `timing`, measurements, coverage
reasons, and accounting scopes remain available in the nested evidence.

Rows preserve candidate, logical and resolved run, attempt slot, observation
slot, attempt, and retry identities. `provenance` and `evidence` retain the
original documents and coordination, including recorded component seeds,
pairing and scheduling blocks, run order, warm-up reports, timeout limits,
machine context, worker environments, and observed controls. Missing historical
evidence stays null. Export does not resolve missing provenance on the current
host. Schema references come from the recorded result and metric envelopes.
The current one-shot contract does not record metric unit or direction
annotations; their columns remain null.

JSONL uses canonical JSON followed by one newline per row. CSV uses UTF-8,
fixed column order, CRLF record endings, and standard escaping for quotes,
commas, and line breaks. Empty scalar cells mean unavailable values. The
`metric_value` cell always contains canonical JSON, including `null`, quoted
strings, and arrays. Nested `timing`, `compatibility`, `provenance`, and
`evidence` cells likewise contain canonical JSON when available.

## Compatibility and lineage

Every row carries an explicit `compatibility` object and an `export-v1:`
grouping key. The key hashes that object's canonical bytes with SHA-256. It
binds execution-semantics and measurement versions, timing scope, primary time,
environment class, the complete execution policy, observed control requests
and guarantees, and resource-accounting scope and completeness. Missing
classification evidence binds the particular record ID so absence cannot
establish compatibility between unrelated records.

`execute_only` remains distinct from `prepare_and_execute` and
`cold_end_to_end`. Development evidence remains distinct from locked
environments, and best-effort or unsupported controls remain distinct from
enforced controls. The control summary reports the weakest recorded status;
the full list retains each requested control and its scope. `control_evidence`
distinguishes observed controls from declarations retained in preflight failure
records. Declarations alone have unknown observed guarantees. Matching keys do
not establish scientific comparability or equal hardware. Benchmark authors
must also select compatible problems, datasets, parameters, machines, and
measurement evidence before downstream analysis.

`export.json` contains the canonical selector, source candidate, attempt, and
observation identities in deterministic order, recorded result and metric
schema references, complete compatibility classes, export and tool versions,
and the SHA-256 digest of the exact emitted table. The manifest does not hash
itself. This local lineage receipt does not package schemas or upgrade the
original guarantees; portable snapshots belong to the later snapshot slice.

Select exact identities with repeatable `--id` options:

```sh
metewand export --output-dir runs/example --id mw1-attempt-DIGEST \
  --format csv --destination tables/selected
```

Replace the example ID with a complete ID from `status` or `explain`. Selection
accepts candidate, applicability, logical or resolved run, logical or resolved
attempt or observation slot, attempt, and observation IDs. Multiple selectors
are unioned. Selecting an observation retains its producing attempt and that
attempt's other observations; selecting any history retains its candidate's
applicability decisions. Unknown selectors fail before writing outputs.

The export schemas begin at `EXPORT_SCHEMA_VERSION = 1` and join the existing
version-1 public catalog. Existing public schemas, identity transcripts, wire
messages, execution semantics, and the run-store layout remain unchanged:
`PUBLIC_SCHEMA_VERSION = 1`, `IDENTITY_VERSION = 1`,
`WIRE_PROTOCOL_VERSION = 1`, `EXECUTION_SEMANTICS_VERSION = 2`, and
`RUN_STORE_SCHEMA_VERSION = 1`. The canonical index vector lives beside the
schemas in `schemas/v1/fixtures/export-index-v1.jsonl`.
