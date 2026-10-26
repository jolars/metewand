# Native Rust worker helpers

`metewand_runtime::worker_sdk` provides POSIX dataset materializer, one-shot
implementation, and independent evaluator helpers. Workers run as native
executables and need no Python or R process. The helpers reuse the existing version-1 protocol and
canonical JSON implementation.

The crates are not published yet. Add local path dependencies on
`metewand-runtime` and `metewand-protocol`, plus `serde_json`, in the worker's
Cargo project. The [runnable Rust example](https://github.com/jolars/metewand/tree/main/examples/rust-sdk)
builds in this workspace and includes a complete manifest and reference data.
`metewand scaffold dataset|problem|implementation NAME --language rust` generates
an adapter for each role; implementations also require `--problem NAME`. Add the
adapter as a binary target in your Cargo project and copy its executable beside
the `.rs` file, using the same basename without an extension. See
[component scaffolding](authoring-cli.md#component-scaffolding) for source bundles
and the scientific declarations to complete.

## Dataset materializers

Call `serve_dataset_materializer(&mut callback)`. The callback accepts
`&MaterializeRequest`, which exposes `source_dir()`, `dataset_parameters()`,
`dataset_seed()`, and `output_dir()`. It constructs and converts the scientific
data, writes files beneath the assigned output directory, and returns a
`WorkerResult<String>` containing the relative completed manifest path.

Use `write_json(path, data)` for canonical JSON files and
`write_dataset_manifest(directory, &[("data.json", "application/json")])` to
hash completed files and write the standard inventory last. Entries have unique
relative paths and author-selected media types. The helper sorts entries by
path, rejects unsafe paths and symbolic links, and records file sizes and
SHA-256 digests. A custom manifest writer can add representation metadata.
The SDK verifies that the returned manifest exists before responding; the
runtime validates the full artifact against the selected dataset schema.

## Implementations

Implement `Implementation`, then call `serve_implementation`:

```rust
use metewand_protocol::PrepareRequest;
use metewand_runtime::worker_sdk::{Implementation, WorkerResult, serve_implementation};
use serde_json::{Value, json};

#[derive(Default)]
struct Solver { offset: i64 }

impl Implementation for Solver {
    type Output = Value;

    fn prepare(&mut self, request: &PrepareRequest) -> WorkerResult<()> {
        self.offset = serde_json::from_value(request.problem_parameters()["offset"].clone())?;
        Ok(())
    }

    fn execute(&mut self) -> WorkerResult<Value> {
        Ok(json!({"answer": 6 + self.offset}))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    serve_implementation(&mut Solver::default())?;
    Ok(())
}
```

`PrepareRequest` exposes the private dataset directory, problem contract
identity, resolved problem and implementation parameters, and implementation
seed. Preparation must establish clean state. Helpers do not set random
generators, load datasets, or choose scientific representations.

`execute` returns scientific data. The default writer serializes it into
`result.json` with the standard result envelope and an empty file inventory.
`Output` can be any Serde-serializable type. Override `result(output)` to convert
solver output to a scientific JSON object. `statistics(&output)` optionally
returns a diagnostic JSON object. Neither callback contributes to SDK solve
timing. Adapters must convert library values deliberately and enforce their
scientific finite-value rules; helpers validate the resulting canonical JSON
domain and require result and metric data to be objects.

Override `write_result(output, directory)` for custom artifact files. Return a
normalized relative manifest path after closing all files. The SDK checks that
the manifest exists beneath the assigned directory. Metewand independently
checks the full inventory, schemas, sizes, and hashes before publication.
`read_json`, `write_json`, `write_result`, and `write_metrics` provide canonical
JSON and standard envelopes for custom callbacks. Standard envelopes omit a
schema reference so the selected problem contract supplies the schema.

Execution consumes preparation even when solving, statistics, conversion, or
writing fails. Override `reset` to support process reuse; the default returns
`unsupported_operation`, causing the runner to discard the process. A failed
preparation or reset leaves no prepared execution. `shutdown` optionally
releases resources and succeeds by default.

## Independent evaluators

Pass a function or closure to `serve_evaluator`. Its `EvaluateRequest` includes
dataset and result directories, the contract identity, resolved problem
parameters, the exact assigned metrics path, and scientific rules:

```rust
use metewand_protocol::{EvaluateRequest, EvaluationVerdict};
use metewand_runtime::worker_sdk::{Evaluation, WorkerResult, read_json, serve_evaluator};
use serde_json::json;
use std::path::Path;

fn evaluate(request: &EvaluateRequest) -> WorkerResult<Evaluation> {
    let result = read_json(&Path::new(request.result_dir()).join("result.json"))?;
    let answer: i64 = serde_json::from_value(result["data"]["answer"].clone())?;
    let error = (answer - 8).abs(); // This example's independent scientific reference.
    let rules = request.problem_semantics().expect("negotiated scientific rules");
    let target: i64 = serde_json::from_value(rules["one_shot_completion"]["absolute_error"].clone())?;
    Ok(Evaluation::new(json!({"absolute_error": error}),
        if error <= target { EvaluationVerdict::Reached } else { EvaluationVerdict::NotReached }))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    serve_evaluator(&mut evaluate)?;
    Ok(())
}
```

The SDK advertises `problem_semantics` and requires the rules object in each
evaluation request. Only evaluator code computes metrics and selects
`Invalid`, `Reached`, `NotReached`, or `NotRequired`. The helper writes the
standard metrics envelope at the assigned path before sending the verdict.
Implement the `Evaluator` trait to override `write_metrics` when necessary.

## Timing, transport, and errors

`implementation_time_ns` uses a monotonic clock around only `execute`.
Preparation, statistics, scientific conversion, artifact writing, and protocol
encoding fall outside that diagnostic interval. Durations outside the safe
integer range become `null`. Executor-observed timing remains authoritative
and includes result serialization in every scope containing execution.

The helpers read `METEWAND_PROTOCOL_READ_FD` and
`METEWAND_PROTOCOL_WRITE_FD`, duplicate the descriptors, and close only their
copies on return. Standard output and standard error remain available for
logs. `serve_implementation_on` and `serve_evaluator_on` also accept explicit
streams, including borrowed streams. There is no automatic stdin/stdout
fallback. Handshakes report SDK name `metewand-rust` and the workspace package
version. Implementations advertise only `one_shot`.

Return `WorkerError::operation_failed(message)` for ordinary failures, or
convert an existing `WorkerFailure` into `WorkerError` to preserve its stable
code and optional object details. I/O, JSON, and canonicalization errors support
`?`. Unwinding callback panics become `operation_failed` and the panic hook
writes details to stderr. Oversized failures become bounded `internal_error`
responses. Malformed frames, missing IDs, early EOF, invalid inherited
descriptors, and pipe failures terminate serving with `ServeError`.

No compatibility version changes: these callbacks produce the existing wire
messages and result and metrics envelopes. Focused tests cover native processes,
canonical artifacts, callback timing, lifecycle failures, malformed requests,
and independent evaluation through the public CLI.
