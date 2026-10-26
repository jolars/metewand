# Build evidence

The local runner records SHA-256 hashes of its own executable and each resolved
worker program. It looks for an adjacent `PROGRAM.metewand-build.json` receipt
and attaches its build evidence only when the receipt validates against
`build-evidence` version 1 and its executable hash matches the bytes being run.

Capture a build from the repository root:

```sh
python3 tools/build_evidence.py \
  --executable target/release/metewand \
  --source metewand=. --compiler rustc --profile release \
  --lockfile Cargo.lock -- \
  cargo build --release --locked -p metewand-cli --bin metewand
```

The wrapper observes source state, compiler identity, and native lockfiles before
running the command, checks them again after success, and atomically writes the
receipt beside the resulting executable. It rejects changed inputs and failed
builds. Build outputs and receipts must be ignored by Git or written outside the
source repositories. Copy the receipt alongside a staged executable, keeping
the suffix `.metewand-build.json` after its new filename. The receipt need not
belong to the worker's declared source bundle.

Repeat `--source NAME=PATH` for every source repository used by the build and
`--lockfile PATH` for each native lockfile. Source evidence includes the commit,
dirty status, and a versioned SHA-256 transcript of sorted tracked and unignored
files, including executable bits, symbolic links, deletions, and submodule
worktrees. A submodule records its expected Git commit and its observed source
state; an uninitialized submodule has a null observed source. Ignored files are
outside this transcript. Include additional build inputs explicitly in a source
repository rather than assuming Git identifies them.

The receipt records the exact command, working directory, compiler path, compiler
executable hash, compiler version output, profile, requested features, relevant
compiler and linker environment flags, and lockfile hashes. Repeat `--features`
for the selected feature names. Profile and feature labels come from wrapper
arguments (`settings_origin: "wrapper_arguments"`); the command and captured
environment retain the actual invocation. Declare the same settings in both
places. These local observations do not authenticate the builder or prove an
undeclared dependency closure.

Attempts and observations retain evidence under
`provenance.data.context.runner` and
`provenance.data.context.implementation.executable` or
`provenance.data.context.evaluator.executable`. Each contains
`executable_sha256` and a `build` object with `status: "available"` and the
original `evidence`, or `status: "unavailable"` and a reason. Missing receipts
use `not_recorded_at_build`; malformed, oversized, unreadable, or mismatched
receipts remain unavailable. Hashing a Python or R launch program identifies
that interpreter. It does not manufacture build evidence for imported native
libraries or programs selected later by an adapter.

`context.host_observation` separately labels runtime observations with
`phase: "run_preflight"`. It records the current OS and architecture and,
where available, Linux kernel, CPU model, and total memory. These observations
describe the execution host. Metewand never queries the current host compiler
to fill missing build fields or retroactively upgrades old records.

Build receipts supplement provenance. They do not change logical identities,
resolved identities, execution semantics, environment classes, or control
guarantees. [Inspection](inspection-cli.md) and both [exports](exports.md)
retain the original evidence after the source tree and executables are gone.
The [adopter qualification](https://github.com/jolars/metewand/tree/main/examples/adopters)
and [release overhead example](https://github.com/jolars/metewand/tree/main/examples/overhead)
use this workflow for native workers and the runner.
