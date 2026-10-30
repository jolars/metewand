# Contributing

This is the contributor guide for Metewand. The [design](docs/design.md)
describes the general model, [TODO.md](TODO.md) lists remaining work by area, and
[AGENTS.md](AGENTS.md) gives repository instructions. User workflows belong in
the [guide](docs/guide/overview.md).

Keep this document focused on implementation decisions and how to verify them.
The Rust types, checked-in schemas, SQL layouts, package help, and conformance
fixtures specify individual interfaces without a parallel catalog of reference
pages.

## Development environment

Use Rust 1.98.0 and edition 2024. `devenv shell` provides the pinned toolchain,
rustfmt, Clippy, Nextest, Python, R with `jsonlite`, `roxygen2`, `testthat`, and
`withr`, and Node.js:

```sh
devenv shell
test-fast
test-rust
```

`test-fast` runs core and protocol tests. `test-rust` runs the complete Rust
suite. Both accept Nextest filters. The Python package requires Python 3.11 or
later; the R package requires R 4.2 or later and `jsonlite`, with no native
compilation step. The website needs Node.js
22.18 or later.

The local runner supports Linux, macOS, and native Windows x64. On Windows,
install Rust's MSVC toolchain, Visual Studio C++ build tools with the Windows
SDK, and Python on `PATH`. The R package itself requires neither Rtools nor a
C++ compiler. See the [source installation guide](docs/guide/getting-started.md)
for PowerShell commands. Nix and devenv are optional on other systems.

Use local path dependencies for development, and install language packages into
the environment selected for the worker.

The R package uses roxygen2 comments in `packages/r/R/` to generate its help
pages and exports. After changing those comments or public function signatures,
regenerate the documentation from the repository root:

```sh
Rscript --vanilla -e 'roxygen2::roxygenise("packages/r")'
```

Commit the generated `packages/r/man/*.Rd` files and `packages/r/NAMESPACE`
alongside the source comments. These files let users install the package without
roxygen2. Do not edit them directly.

The R package tests use testthat edition 3. Run them from the repository root:

```sh
Rscript --vanilla -e 'testthat::test_local("packages/r")'
```

The reusable [R package workflow](.github/workflows/r-package.yml) uses
`r-lib/actions` to install dependencies from `DESCRIPTION` and run `R CMD check`
on the built source package. It checks the current R release on Linux, macOS,
and Windows, plus R-devel and the previous release on Linux. Package warnings
fail CI, and failed checks retain their logs and testthat output. The Linux
release job also checks that the generated documentation is current.

The Rust test jobs install the R package and `jsonlite` into the interpreter's
site library so isolated CLI workers can load them. These jobs exercise the
worker lifecycle and native R starter separately from the package checks.

## Architecture and source map

  | Location                                 | Responsibility                                                                                                       |
  | ---------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
  | [Core](src/core)         | Manifest parsing, contracts, schemas, canonical values, identities, seeds, planning, scheduling, and public records. |
  | [Protocol](src/protocol) | Wire types, role and capability negotiation, and bounded framing.                                                    |
  | [Runtime](src/runtime)   | Source resolution, artifacts, environments, executors, measurements, provenance, publication, and the run store.     |
  | [CLI](src/cli)           | Argument parsing, orchestration, diagnostics, and human and machine rendering.                                       |
  | [Schemas](schemas/v1)                    | Versioned external contracts and adjacent fixtures.                                                                  |
  | [Shared fixtures](fixtures)              | Cross-language canonicalization, identity, seed, and protocol vectors.                                               |
  | [Packages](packages)                     | Python and R workers; native Rust helpers live in the runtime.                                                       |
  | [Examples](examples)                     | Runnable end-to-end comparisons and reproduction commands.                                                           |
  | [Website](docs)                          | User guides and the published general design.                                                                        |

Metewand is one binary-only Cargo package. Core and protocol are independent
internal modules. Runtime may use both; the CLI composes all three. Keep
filesystem and execution concerns out of core and protocol, and domain types
out of the CLI.

## Manifests and planning

Manifest parsing produces typed declarations with normalized relative paths,
source spans, and deterministic diagnostics. Lower compact field declarations,
flat contracts, scalar axes, function launches, and inferred defaults into the
same effective model used by explicit declarations. Reject competing spellings
for one boundary. Diagnostic source locations do not enter identities.

Compact `parameters` generate closed object schemas and literal defaults or
expansion grids. Fields require types; they are required unless optional. Result
and metric fields never receive defaults. Declaration grids select values
without restricting the valid domain. Experiment axes replace matching
declaration grids; cases replace matching experiment axes. Expanded values merge
recursively with literal defaults before validation.

Resolve schema references through the offline catalog. Never retrieve schemas
from the network. Inline schemas inherit JSON Schema 2020-12 before hashing.
Resource identities include resolved dependencies; moving a file-backed schema
inline can change a path-bearing identity. Equivalent compact and explicit
normalized declarations share their identity path.

Keep configuration expansion and scheduling assertions in core tests. Logical
planning is pure: callers supply resolved definition identities through the
planning catalog. Resolution later binds dataset content, environments, worker
launches, and applicability evidence. Declared capabilities are not verified
capabilities.

The built-in empty dataset has a versioned definition and schema. Omitted
parameters form a closed empty object. Unambiguous function benchmarks default
to a local environment, one-shot execution, one measurement, and wall time over
`prepare_and_execute`. Profile selection stays explicit.

Sequential scheduling follows plan order. Randomized scheduling interleaves
implementation configurations within dataset, problem,
implementation-repetition, and measurement blocks. Bounded concurrency
contributes to execution policy and resolved identity. Dispatch and event order
remain deterministic, and batches do not cross experiment boundaries.

## Canonical values and identities

Metewand hashes and transports one restricted JSON domain. Object keys must be
unique after escape decoding. Strings contain Unicode scalar values, without
normalization. Numbers must be finite binary64 values; integral values must lie
within `[-9007199254740991, 9007199254740991]`. Normalize negative zero to zero
and safe integral float spellings to integers. Reject invalid UTF-8, byte-order
marks, lone surrogates, duplicate keys, and nonfinite numbers.

Canonical bytes follow RFC 8785: no insignificant whitespace or trailing
newline, recursive UTF-16 property ordering, and the specified number and string
encoding. Parse, validate against the selected schema, then request canonical
bytes explicitly. An ordinary JSON serializer is not a canonicalization API.

Typed identities use domain-separated canonical records and an
`mw1-<kind>-<digest>` representation. Treat their dependency graph as part of
the contract. Logical identities bind requested comparisons; resolved identities
also bind actual content, launches, and execution evidence. Allocated paths,
wall-clock timestamps, and output destinations do not define those identities.
The manifest hash binds the normalized typed manifest, not YAML source bytes.

### Seeds

For role `dataset`, `implementation`, or `scheduling`, derive:

```text
SHA-256("metewand-seed-v1" || NUL || UTF-8(role) || NUL || canonical_fields)
```

  | Role           | Canonical fields                                                                                 |
  | -------------- | ------------------------------------------------------------------------------------------------ |
  | Dataset        | `dataset_definition`, `dataset_parameters`, `experiment_seed`                                    |
  | Implementation | `dataset_configuration`, `experiment_seed`, `implementation_repetition`, `problem_configuration` |
  | Scheduling     | `benchmark_name`, `experiment_name`, `experiment_seed`, `scheduling_policy_version`              |

The exposed seed is the first 53 digest bits, most significant first. Retain the
full digest as evidence. Omitting implementation identity deliberately pairs
methods within a comparison block. Packages deliver the seed; benchmark code
seeds its own generators.

### Local trees

Local-tree hashing sorts normalized paths by UTF-8 bytes and binds file bytes,
executable flags, empty directories, and exact symbolic-link target spellings.
It omits the root name, timestamps, ownership, and other permission bits.
Hashing a symlink does not authorize following it during execution.

The version-1 SHA-256 transcript is:

```text
"metewand-local-tree\0" || u32(version) || u64(entry_count) || entries
directory = bytes(path) || "d"
file      = bytes(path) || "f" || executable_u8 || u64(size) || file_bytes
symlink   = bytes(path) || "l" || bytes(target)
```

Integers are unsigned big-endian; `bytes(x)` is `u64(length) || x`. The root is
omitted, and the executable byte is zero or one. Preserve exact vectors when
changing canonical or hashing code. See [identity
types](src/core/identity.rs), [seed
derivation](src/core/seed.rs), [tree
hashing](src/runtime/local_tree.rs), and [fixtures](fixtures).

## Compatibility versions

The Rust binary package has its own SemVer. Python and R releases are
independent and managed by Versionary from Conventional Commits. Binding API
version 1 governs package compatibility with the CLI; equal package and CLI
release numbers are unnecessary. It is separate from public compatibility versions:

  | Boundary                                                      | Current version                                                |
  | ------------------------------------------------------------- | -------------------------------------------------------------- |
  | Manifest, public schemas, wire protocol, and typed identities | 1                                                              |
  | Language binding API                                        | 1                                                              |
  | Seed derivation and scheduling policy                         | 1                                                              |
  | Execution semantics                                           | 2                                                              |
  | Measurement provenance                                        | 3 for streaming, 2 for other deliveries; versions 1 and 2 remain readable |
  | Worker environment provenance                                 | 1 on Unix, 2 on Windows                                        |
  | Exports and build evidence                                    | 1                                                              |
  | Portable result archives                                     | 1                                                              |
  | Prepared environment lockfiles                               | 2; version 1 Nix remains readable                                                              |
  | SQLite layout                                                 | 1 for one-shot, 2 for buffered profiles, 3 for fresh sequences and streaming |

The binding API compatibility constant lives with the runtime adapters.
The constants live with their authoritative types in core, protocol, and
runtime. Public JSON Schemas declare their dialect and compatibility version.
Version externally consumed boundaries; internal helpers stay private, and
nested records normally inherit their enclosing format's version.

Before the MVP, authoring syntax and profile APIs may evolve with their current
consumers. Pin a revision for retained benchmarks. Preserve supported persisted
records and canonical identity meanings. Never silently reinterpret historical
results, combine incompatible timing scopes, or reuse an identity for different
semantics. Use the affected version, deliberate migration, or an unsupported
version diagnostic when required. Negotiate protocol extensions by capability or
a mutually supported version.

Consider only boundaries the change affects. Briefly record consequential
compatibility decisions in the change description and update the relevant
fixtures. Do not require a separate review document or tests for speculative
interfaces.

## Worker protocol and packages

The POSIX launcher supplies separate binary pipes through
`METEWAND_PROTOCOL_READ_FD` and `METEWAND_PROTOCOL_WRITE_FD`. Standard output
and standard error remain log channels. There is no automatic stdin/stdout
fallback. Native helpers emit canonical JSON Lines. Receivers accept ordinary
JSON in the supported value domain, with an inclusive 1 MiB limit
per frame including its final line feed. Responses must match the pending string
request ID. Malformed frames and unusable sessions terminate serving.

The Windows launcher supplies dedicated synchronous worker pipe handles through
`METEWAND_PROTOCOL_READ_HANDLE` and `METEWAND_PROTOCOL_WRITE_HANDLE`. It limits
handle inheritance and assigns workers to a kill-on-close Job Object at
creation. Parent endpoints support cancellation, and deadlines terminate the Job
before bounded cleanup. The native Rust helper duplicates supplied handles into
owned binary streams and prevents further inheritance. Explicit descriptor APIs
remain available to native workers on both platforms.

Language bindings register callbacks through binding API version 1. The CLI
stages its bundled adapters in private worker directories and includes their
hashes in resolved launch identities. Installed packages contain scientific
values, JSON utilities, and delegates; they do not own protocol state or profile
policies. R requires only `jsonlite`, and Python uses its standard library.

Managed function launches automatically use authenticated loopback sockets.
Lifecycle package workers select `protocol-transport: socket` explicitly.
Endpoints bind to 127.0.0.1 on ephemeral ports, use per-worker authentication,
and share the hello deadline and cancellation path. Ports and tokens do not
enter identities or provenance; tokens are filtered from logs. Standard output
and standard error remain independent logs. Native workers retain pipe transport.

The `managed_outputs` capability admits version-1 output drafts with scientific
data and unhashed artifact declarations. The runtime finalizes these into the
existing canonical envelopes before the operation finishes. All serialization,
hashing, and finalization remain inside the selected timing scope. The runtime
then independently validates completed artifacts as before. R's secondary
in-worker solve time is unavailable rather than approximated.

A hello exchange negotiates protocol version, role, capabilities, identity, and
package metadata (the existing `sdk` wire field). The Rust `SdkMetadata` type
and `worker_sdk` module retain their API names for compatibility.
Implementations prepare and execute, with optional reset and shutdown hooks.
Materializers and evaluators serve separate roles. Evaluators that negotiate
`problem_semantics` receive the resolved scientific rules. Applicability, fresh
sequences, buffered execution, and streaming require their respective capabilities;
one-shot messages retain their supported shapes.

Worker failures use `invalid_request`, `unsupported_operation`,
`operation_failed`, or `internal_error`, a nonempty message, and optional object
details. Preserve the failing phase. Operational timeouts, early EOF, protocol
violations, and failed resets discard the worker. Scientific deadlines remain
distinct from phase deadlines.

For callback signatures, consult the [Python
package](packages/python/src/metewand), [R help pages](packages/r/man), and
[Rust helpers](src/runtime/worker_sdk.rs). The [package
examples](examples) exercise each role. Ordinary functions use contexts and JSON
objects; lifecycle adapters separate preparation, solving, conversion, custom
writing, reset, and shutdown.

Preparation must establish clean state. Execution consumes preparation even when
a callback fails. Reuse requires a successful reset rather than an assumed clean
worker. Package solve timing excludes conversion and artifact writing;
executor-observed timing remains authoritative. Close every output file before
replying. The runtime independently validates schemas, manifests, inventories,
sizes, hashes, and path containment before publication.

The CLI-owned adapter conformance suites consume the shared fixtures directly.
The installed binding packages test their stable values, registration interface,
and serialization utilities. Keep adapter canonicalization and protocol behavior
aligned with Rust. Add callback coverage
where an interface is implemented; every package need not support every profile
callback before an adopter can use one.

## Sources and artifacts

Resolve declared sources relative to the manifest, expand globs
deterministically, and preserve normalized bundle paths. Function references
include their selected file; imported modules and other inputs require
declarations. Relative worker programs resolve within the bundle. Bare programs
use the selected `PATH`. `{sources}/PATH` is an explicit argument reference, not
general interpolation.

The local launcher uses an environment allowlist, fixed locale and timezone,
and private working, home, temporary, configuration, and cache directories.
Do not inherit ambient language search paths. Resolved launch
identities use stable path templates and content hashes; provenance records
allocated paths. The local environment does not establish a dependency closure
or isolation from undeclared host files.

`metewand lock` prepares the complete plan's worker graph. Nix evidence binds
content snapshots, native locks, source NARs, realized outputs,
runtime closure hashes and references, and normalized launches. Root snapshots
exclude `metewand.lock` to avoid self-reference. Git snapshots use tracked
working-tree contents; non-Git snapshots include the repository tree. Keep
outputs outside captured sources. Local flakes receive content snapshots rather
than Git revision metadata. Native dependency locks remain author-owned.

Locked preflight checks actual NAR contents in addition to database hashes.
It never repairs, fetches, or builds missing paths. Temporary GC roots protect
prepared paths for the invocation. All launched worker roles use the same
resolver, with no ambient host executable fallback. Lock evidence contributes
to resolved environment identities; the whole lockfile digest is provenance,
so unrelated entries do not define worker identity. Locked runs retain the
trusted local executor and its reported control limitations. Existing
development identities, execution versions, and SQLite layouts remain unchanged.

The normal suite tests closure verification through an injectable Nix command
boundary. With Nix and uv available, run the Nix and mixed-backend integration tests explicitly:

```sh
cargo test --test integration --locked cli_tests::locked_nix:: -- --ignored
```

uv preparation shares worker discovery and lock publication with Nix. The uv
command boundary is injectable for focused tests. Prepared generations live
outside source snapshots, use non-editable installs and copied package files,
and retain separate raw integrity receipts and normalized identity evidence.
Verification reads installed files without invoking uv or importing packages.
New locks use version 2; the version-1 Nix serializer and fingerprint remain
unchanged. Install uv to run the offline package integration cases explicitly:

```sh
cargo test --test integration --locked cli_tests::locked_uv:: -- --ignored
```

Unix workers use umask `0077` and retain worker-environment provenance version 1.
Windows records version 2 with observed ACL evidence, required Windows system
variables, and no Unix umask. Environment names cannot conflict when compared
without ASCII case on Windows. Logical source and artifact paths retain `/`;
Windows filesystem operations reject reserved names, aliases, alternate data
streams, case collisions, and unsupported reparse points. Python launch
resolution preserves the selected virtual environment, including its Windows
`Scripts` directory.

Materialization separates source acquisition, derivation identity, and output
content. Construct a dataset configuration once per run, or reuse its verified
persistent mapping. Every attempt receives a verified private copy. Verify
copies and canonical dataset objects around worker operations. Detected mutation
prevents observation publication and quarantines corrupt cached content.

Persistent caches use content-addressed objects, derivation mappings, leases,
and durable roots. Collection requires exact explicit targets and must preserve
all live leases and roots, including failed-run dependencies. Removing a run
directory does not release its cache roots. Inspection and dry runs do not
initialize, recover, or repair a cache.

The optional [Datamonger example](examples/datamonger) uses a provider process
for pinned external dataset resolution. It exercises runtime APIs; the manifest
does not currently select external providers or optional binary dataset
checkers.

## Execution and profiles

Keep the three timing envelopes, validity, completion, and applicability defined
in the [design](docs/design.md) separate in records and reports. Shared dataset
construction and independent evaluation stay outside the primary interval.
Continuous cold and prepare-and-execute timing includes integrity and durable
coordination between preparation and execution; provenance labels that overhead.

Measurement payloads distinguish `complete`, `partial`, and `unavailable`.
Nanoseconds and bytes are safe integers. Linux CPU and peak-RSS evidence is
partial because it covers the implementation process without descendants. Peak
RSS covers its lifetime through execution, even for execute-only timing.
CPU and peak-RSS evidence is unavailable on macOS and Windows. Historical
unavailable fields remain unavailable.

Streaming measurements use provenance version 3 and retain active, elapsed,
paused, and evaluation time separately. Linux streaming CPU includes sampled
process-group members and remains partial because short-lived or detached
children may be missed. Windows streaming CPU uses Job Object totals, including
terminated children. macOS streaming CPU and non-Linux peak RSS remain
unavailable. Accounting excludes checkpoint pauses, and its source and coverage
remain explicit. Other delivery modes retain their version-2 accounting.

Fresh sequences reserve cumulative time per repetition, commit stopping
decisions with accepted points, and retain skipped slot dispositions. Resume
preserves accepted points and budget evidence. Unknown interrupted time retains
its reserved charge. Clean-state reruns are not continuation from checkpoints.

Buffered profiles bound trace points, snapshots, total bytes, terminal reserve,
and active time. Retain local metrics as provisional evidence and independently
evaluate snapshots. Named instrumented scopes must be authorized by the problem.
Partial observations do not satisfy a retry, and acceptance selects observations
from one producing attempt. Never splice trajectories across retries.

Streaming profiles negotiate `streaming_profile` within wire protocol 1. One
`execute_stream` request remains pending through contiguous checkpoint and
decision sequences. Each snapshot uses a distinct contained directory, and its
manifest and files must fit `max-checkpoint-bytes`. A scheduled checkpoint or
explicit terminal checkpoint consumes one observation. Trusted evaluator metrics
determine threshold crossings and stopping; provisional worker metrics do not.

Pause the active clock after checkpoint serialization and managed-output
finalization. Validate and freeze the snapshot, independently evaluate it, and
commit the observation and decision in one fenced transaction after artifact
publication. Only then send the decision. Resume active wall and process-tree
CPU accounting before `continue`; exclude the final `stop` acknowledgment and
bound it with the shutdown deadline. The execution-policy timeout and profile
active bound apply to cumulative active intervals, while execute deadlines bound
individual exchanges and evaluation retains its own deadline. Cooperative
adapters must quiesce all solver work while waiting.

Streaming reuses layout 3 without migrating historical stores. Committed
checkpoints survive later failure, but a saved `stop` decision does not prove
worker acknowledgment or terminal success. Successful finalization selects one
attempt's trajectory atomically. Resume skips accepted streams; an explicit
retry starts a new clean trajectory. The capability extension and distinct
delivery identity preserve existing identity vectors and execution semantics.

## Build evidence

The runtime accepts an adjacent `PROGRAM.metewand-build.json` receipt only when
its version-1 schema and executable hash validate. Capture evidence during the
build rather than querying a compiler after a run:

```sh
python3 tools/build_evidence.py \
  --executable target/release/metewand \
  --source metewand=. --compiler rustc --profile release \
  --lockfile Cargo.lock -- \
  cargo build --release --locked -p metewand --bin metewand
```

The wrapper observes sources, compiler, command, settings, relevant flags, and
lockfiles, rejects changed inputs or failed builds, and writes a matching
receipt. Repeat `--source`, `--lockfile`, and `--features` as needed. Keep
outputs ignored or outside the source repository. Stage receipts alongside
renamed executables with the same suffix. These observations do not authenticate
the builder or prove a complete dependency closure.

Runtime host observations remain separate from build evidence. Missing or
invalid receipts remain unavailable, including in historical records. Hashing an
interpreter does not establish build evidence for imported native libraries.

## Durability and recovery

The authoritative layouts are
[v1.sql](src/runtime/run_store/v1.sql),
[v2.sql](src/runtime/run_store/v2.sql), and
[v3.sql](src/runtime/run_store/v3.sql). Readers and writers
check exact supported schemas. Opening an older store never silently migrates
it. Use a new run directory when a selection needs a newer layout.

Connections require WAL, full synchronization, foreign keys, recursive triggers,
and strict tables. Initialization probes filesystem locking, synchronization,
atomic no-replace publication, and SQLite behavior. Unsupported durability
requirements fail rather than falling back to weaker guarantees. Network
filesystems are unsupported. Windows writable roots must be on local NTFS.

Reserve and transition attempts in short immediate transactions with ownership,
lease, and expected-revision checks. Never hold a transaction across worker
execution, hashing, synchronization, or rename. Leases last 30 seconds and renew
every five seconds. Expired owners require explicit recovery.

Publish complete immutable artifacts before terminal acceptance. Synchronize
files and, on Unix, directories, and use atomic no-replace rename. Windows
flushes owned files through writable handles and publishes on the destination
volume with no-replace `MoveFileExW`; it does not claim Unix directory
synchronization. Storage evidence records this behavior for successful and
failed attempts. Recovery on Windows covers abrupt process death, without a
power-loss guarantee. Register only verified artifacts. Finalization claims
coordinate competing successful attempts; one
wins acceptance, and another can retain a superseded valid observation. Report
success only after the terminal transaction commits.

The run directory contains `run.sqlite3`, coordination locks, and immutable
artifact directories. Reports supply relative paths; consumers must not invent
paths from record IDs. `attempt.json`, observation documents, complete
inventories, and coordination `outcome.json` retain the evidence required for
recovery. Directories alone never prove acceptance.

Recovery verifies claims and published outcomes, preserves accepted selections,
and records interrupted work without manufacturing success. Repeating recovery
is idempotent. Retries allocate distinct indices and attempt IDs. Buffered
and streaming acceptance select one trajectory atomically; fresh acceptance also
settles its budget and stopping effect.

Inspection copies a stable database and WAL to private storage and reads a
committed snapshot. It never opens the original through a mutating initializer,
recovers leases, checkpoints, or quarantines artifacts. Indexes and exports
cannot establish acceptance or authorize retention changes.

### Crash conformance

Normal tests include representative publication crashes and focused recovery
checks. Run the exhaustive campaign when changing durability, publication,
recovery, or the run-store schema:

```sh
cargo test --test integration --locked \
  runtime::run_store::fault_tests::every_durability_boundary_survives_process_death_and_resume \
  -- --exact --ignored --nocapture
```

For cache catalog durability, publication, or collection changes, also run:

```sh
cargo test --test integration --locked \
  runtime::artifact_store::maintenance::tests::collection_recovers_after_each_database_sync_rename_and_cleanup_boundary \
  -- --exact --ignored --nocapture
```

The [durability workflow](.github/workflows/durability.yml) runs the exhaustive
campaigns separately on Linux, macOS, and Windows. They interrupt subprocesses
at application and SQLite boundaries and verify recovery, identity, acceptance,
and retry invariants. They
model abrupt process death, not storage hardware losing power.

## Machine output and exports

Human and machine output use the same typed records. Machine formats use the
versioned `machine-output` envelope; JSONL run events flush in order, while JSON
returns a terminal envelope. Worker logs do not enter the command stream. Use
`metewand schema NAME` to inspect an embedded schema offline.

Export reads committed evidence and verified artifacts. A disposable
`index.jsonl` never becomes coordination state. Explicit `--rebuild-index`
replaces only that projection. Table destinations are new directories with
`results.jsonl` or `results.csv` and a lineage manifest, `export.json`.

`--format json` writes the version-1 analysis snapshot, `results.json`, and the
same lineage manifest. It projects complete observation metric objects, shared
recorded provenance, effective settings, timing coordinates, attempt outcomes,
and registered measurement progress. Its receipt counts observation objects as
rows. `--require-complete` validates the registered scope of selected candidates
before any write, using accepted selections and profile progress rather than the
absence of historical failures. It accepts target-based skips and successful
retries while preserving intermediate observations and exclusions.

Capture checkout revision and dirty status, host observations, runner evidence,
and repetition settings during run preflight. Analysis exports use only recorded
evidence; they must remain deterministic after the source checkout or producing
executable disappears. These additions preserve canonical identity rules,
execution semantics, and SQLite layouts. Existing tidy exports retain version 1.

Tidy rows preserve attempts, observations, applicability, trace evidence, and
fresh slot dispositions. Metric names are JSON pointers; entities without
metrics remain present. CSV uses fixed columns, UTF-8, CRLF endings, and
canonical JSON for metric and nested values. JSONL uses canonical bytes and a
newline. Do not aggregate repetitions or discard failures during export.

Compatibility classes bind measurement and execution versions, timing scopes,
environment class, policies, controls, and accounting evidence. Missing
classification binds the particular record rather than establishing
compatibility with unrelated records. Matching classes do not establish
scientific comparability or equal hardware. The export manifest records lineage
and the exact table hash; it is not a portable result archive.

### Portable result archives

`archive create` reads one committed snapshot whose started attempts are all
terminal. It retains pending slots, applicability decisions, immutable attempt
and transition history, observations and selections, artifact relationships,
and fresh-sequence policies and accounting. It copies only registered artifacts.
It does not recover the source, gather external source or input bytes, or include
SQLite coordination files, indexes, staging directories, or orphan files.

The version-1 archive is a gzip-compressed tar file containing `archive.json`,
canonical `evidence.json`, and payloads beneath `data/`. The public schemas and
core archive types define those boundaries independently of SQLite layouts.
The manifest inventories every other file by normalized path, SHA-256, size,
and original executable bit. The original bit remains evidence even on hosts
that cannot represent it. Artifact verification uses it without changing the
version-1 local-tree transcript. Canonical records, ordered members, fixed tar
headers, and fixed gzip metadata make repeated creation deterministic for the
same evidence and writer version.

`archive verify` accepts either an archive file or an imported collection.
Verification checks supported versions, canonical identities, attempt history,
selection relationships, fresh accounting, and exact artifact inventories and
hashes. It does not establish producer authenticity or rerun scientific
evaluation. Extraction rejects links, special entries, explicit directory members,
duplicate or unsafe paths, trailing payloads, and exceeded limits. Verification
and import default to one million file entries and 1 TiB of expanded payload,
including metadata; `--max-entries` and `--max-bytes` override those limits.
Paths that the target filesystem cannot represent fail without renaming evidence.

Creation and import use private staging and atomic publication without replacement.
Their destinations need existing parents. Archive files must be outside their
source run or collection. Import publishes a collection only after verification
and synchronization. The shared reader supplies the same inspection and export
projections for SQLite runs and collections. Collections reject run-store
initialization and index rebuilding; table exports must use a new destination
outside the collection. No SQLite layout or scientific identity version changes.

## Verification

Prefer a focused failing test before implementation. Assert exact bytes where
the contract defines them. Keep combinatorial tests in core and small execution,
record, and provenance cases in the CLI. Run focused checks while developing,
then verify the changed scope once.

Rust changes require:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo nextest run --workspace --all-targets --locked --profile ci
```

CI compiles, runs Clippy, and tests on Linux, macOS, and Windows. The portable
CLI suite exercises Python, R, and Rust workers, generated dataset caching,
fresh sequences, buffered profiles, failures, resume, inspection, and both
exports. Keep Unix-only tests narrowly scoped to Unix operations. R package
contracts and native R starter checks run on all three platforms.

Documentation and website changes require:

```sh
npm --prefix docs ci
npm --prefix docs run deploy:check
```

Documentation-only changes do not require rebuilding or testing Rust. Use
`devenv test` when both scopes change. Keep `Cargo.lock` current and use
`--locked`. The complete agent-loop acceptance test remains opt-in:

```sh
cargo test --test integration --locked cli_tests::agent_loop:: -- --ignored
```

## CLI releases

`dist-workspace.toml` pins cargo-dist 0.33.0 and builds the `metewand` binary
for x64 and ARM64 Linux with glibc and musl, x64 and ARM64 macOS, and native
Windows x64 with MSVC. The CLI opts into distribution through
`package.metadata.dist`.
Archives, SHA-256 checksums, shell and PowerShell installers, and GitHub
attestations go on the root `vX.Y.Z` release. Artifact names use the Cargo
package name `metewand`; the installed command is also `metewand`. Its source
entry point is `src/main.rs`; no library target is published.

Versionary creates root releases as drafts with their changelog notes. The
generated `.github/workflows/v-release.yml` runs on `v`-prefixed version tags,
uploads artifacts, and publishes the draft only after every build succeeds.
Python's `metewand-vX.Y.Z` tags use their own workflow and stay out of GitHub's
latest-release selection so the CLI installers resolve the root release.
Versionary must use `RELEASE_TOKEN`; tags pushed with the default
`GITHUB_TOKEN` do not trigger the binary release workflow.

After editing the distribution configuration, use the pinned cargo-dist version
to regenerate and check the workflow:

```sh
dist generate --mode=ci
dist generate --check
dist plan
```

Do not edit the generated workflow directly. A local `dist build` builds an
archive and installers for the host. Full platform builds run on GitHub's
native runners. Pull requests check the release plan without publishing.

The website serves `docs/public/install` and `docs/public/install.ps1` at
`https://metewand.org/install` and `https://metewand.org/install.ps1`. These
wrappers download the complete latest release installer before running it and
forward arguments. The generated installers use Cargo's binary directory,
normally `~/.cargo/bin`. Worker runtimes and packages are installed separately.
Installer tests run in the platform CI matrix, and website integration tests
check the served bytes and content types.

### crates.io

The binary-only Cargo package is named `metewand`, so users install the CLI with
`cargo install metewand --locked`. The package has no library target or separate
Metewand crate dependencies. Schemas, adapters, SQL files, and authoring templates
ship directly from their canonical source directories. Schema bytes, identities,
and wire compatibility versions do not change for registry packaging.

Check the extracted package before publication:

```sh
compute run -- cargo publish --locked --dry-run
```

CI requires this check before Versionary runs. The integration suite also checks
that Cargo exposes one Metewand package, one binary, and no library target.
Repository tests compile the internal modules directly in
`tests/integration.rs`; the binary's unit-test target stays empty to avoid
running those same tests twice. Native Rust examples also compile internal
modules from the checkout. They do not define a supported Rust library API.

For the first publication, check out the new root release tag, authenticate
with `cargo login`, and publish from that checkout:

```sh
compute run -- cargo publish --locked
```

A published version cannot be replaced. The existing `v0.2.0` tag predates this
package layout; use a new root release containing it.

After the first publication, configure a [Trusted
Publisher](https://crates.io/docs/trusted-publishing) on the `metewand` crate:
GitHub owner `jolars`, repository `metewand`, workflow `crates-publish.yml`, and
environment `crates-io`. Create that GitHub environment with a deployment policy
that permits root release tags. No stored crates.io token is needed in GitHub.

The separate `.github/workflows/crates-publish.yml` runs on root `vX.Y.Z` tag
pushes and supports manual dispatch with an existing tag. It requires the tag
to match the package version, verifies the package, authenticates with Trusted
Publishing, and publishes the CLI. Python and R tags do not trigger it.
Versionary must push tags using `RELEASE_TOKEN` so GitHub starts this workflow.
Registry publication and prebuilt binary builds run independently; inspect both
workflows when releasing.

## Language package releases

Versionary creates release PRs, tags, changelogs, and GitHub Releases in
independent monorepo mode. The root uses the Rust strategy, `packages/python`
uses the Python strategy, and `packages/r` uses the R strategy. Each package
changes version when its own files change; CLI adapter changes belong to the
root release. Python reads its installed version from distribution metadata,
whose source is `pyproject.toml`. R reads its version from `DESCRIPTION`. Worker
hello metadata reports the installed package version. Binding API version 1
governs compatibility with the CLI independently of those release numbers.

Versionary creates root tags `vX.Y.Z`, Python tags `metewand-vX.Y.Z`, and R tags
`metewand-r-vX.Y.Z`, with separate changelogs and GitHub Releases. Prebuilt CLI
artifacts use the root tag. Package tags identify their source revisions and do
not imply a CLI release with the same version.

The reusable `.github/workflows/python-package.yml` builds the source
distribution and a wheel from that distribution, checks both with
`twine check --strict`, and tests each installed artifact on Python 3.11 and
3.14 on Linux, macOS, and Windows. Tests run outside the checkout without
`PYTHONPATH`. CLI adapter tests consume shared conformance fixtures in the
workspace Rust suite. CI requires these checks before Versionary runs. To
reproduce them locally, use a disposable virtual environment:

```sh
python3 -m venv /tmp/metewand-package-test
. /tmp/metewand-package-test/bin/activate
python -m pip install build twine
python -m build --outdir /tmp/metewand-dist packages/python
python -m twine check --strict /tmp/metewand-dist/*
python -m pip install /tmp/metewand-dist/*.whl
metewand_checkout=$PWD
cd /tmp
python -m unittest discover -s "$metewand_checkout/packages/python/tests" -v
```

Repeat in a fresh environment with the `.tar.gz` source distribution instead of
the wheel and with each supported test interpreter.

Before the first publication, configure the external accounts:

1. Create the GitHub environment `pypi`. Its deployment policy must allow
   release tags. Add required reviewers if publication should require manual
   approval.
2. Add a [pending Trusted
   Publisher](https://docs.pypi.org/trusted-publishers/creating-a-project-through-oidc/)
   on PyPI with project name `metewand`, owner `jolars`, repository `metewand`,
   workflow `python-publish.yml`, and environment `pypi`.
3. Ensure Versionary uses `RELEASE_TOKEN` when creating GitHub Releases. Events
   created with the default `GITHUB_TOKEN` do not trigger another workflow.

Publishing a Python GitHub Release triggers
`.github/workflows/python-publish.yml`. Root releases do not publish to PyPI.
The workflow checks out the Python release tag, requires the tag to equal
`metewand-v` plus the Python package version, and runs the distribution checks
before uploading those same artifacts with Trusted Publishing. No PyPI API token is
needed. The workflow can also be dispatched manually with an existing Python
release tag, for example if the release event did not trigger it. PyPI does not
permit replacing previously uploaded files; a corrected package needs a new
version.

Review the release PR, CLI installation instructions, and Python
package README before publishing the first release. The Python package supplies
worker helpers; users install the CLI separately from a compatible root release
or source tag. The R package still requires its own deliberate publication
workflow.

## Website development

The site at <https://metewand.org> uses VitePress. From the repository root:

```sh
npm --prefix docs ci
npm --prefix docs run dev
```

`docs/index.md` is the homepage. `docs/guide/` contains the overview, getting
started, and multipart authoring guide. `docs/design.md` describes the general
design. `.vitepress/config.ts` defines navigation, search, and metadata. Keep
local page links relative with `.md` extensions; link to schemas, fixtures,
source, and runnable examples on GitHub. The build rejects missing page links.

Edit branding originals in `assets/`. Development and build commands copy them
to the ignored `docs/public/brand/` directory. Favicons and `site.webmanifest`
live in `docs/public/`. VitePress generates the sitemap; `robots.txt` points
crawlers to it. Shared metadata supplies social preview images and page URLs.

`npm --prefix docs run deploy:check` builds the site, runs integration tests,
and validates Cloudflare packaging without credentials or uploads. The output is
`docs/.vitepress/dist/`; `npm --prefix docs run preview` previews its HTML.

### Deployment

`docs/cloudflare.config.ts` declares the Worker and custom domain.
`docs/wrangler.config.ts` supplies the assets to the pinned Cloudflare build
tool. The [Website workflow](.github/workflows/website.yml) checks pull requests
and deploys relevant pushes to `main`. It reads `CLOUDFLARE_ACCOUNT_ID` and
`CLOUDFLARE_API_TOKEN` only in its deployment step. The token needs Worker
deployment and custom-domain permissions for the account hosting the zone.

For an authorized manual deployment, authenticate with `npx cf auth login` from
`docs/`, or set those variables, then run `npm --prefix docs run deploy`. Run
the workflow manually on `main` to redeploy a validated build.

### Markdown representations

The build converts each rendered page into Markdown without navigation or
controls. `docs/worker.mjs` serves it when GET or HEAD explicitly prefers
`text/markdown` with positive quality at least as high as `text/html`. Other
requests receive HTML. Both include `Vary: Accept`; Markdown has its own ETag.
Assets, canonical redirects, and missing-page responses retain their behavior.
This does not require Cloudflare's paid Markdown for Agents feature.

After building, start `npm --prefix docs run preview:worker`, then request:

```sh
curl -i -H 'Accept: text/markdown' http://localhost:8787/guide/getting-started
```

The deployment check runs Markdown integration tests against the local Worker
runtime. On NixOS, `devenv shell` sets `MINIFLARE_WORKERD_PATH` to a wrapper
that launches npm's pinned `workerd` through the Nix dynamic loader. Run
`npm ci` before starting it. Outside devenv, provide a compatible executable
through that variable if the npm binary cannot start. The test harness checks
startup, closes even on failure, and bounds test files to 60 seconds.
