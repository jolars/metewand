# R worker helpers

The [R package](../sdk/r) implements the existing version-1 one-shot protocol
for implementations, dataset materializers, and independent evaluators. It
requires R 4.2 or later, POSIX, `digest`, and a C++17 compiler with floating-point
`std::to_chars` support. The native code handles canonical JSON, dedicated pipes,
and monotonic timing. Each worker remains a separate process.

Install the package into the selected worker environment with
`R CMD INSTALL sdk/r`. The local launcher assigns a private `R_LIBS_USER` and
does not inherit ambient language search paths. Install dependencies into the
selected environment's site library, or explicitly bundle an installed SDK as
the [runnable example](../examples/r-sdk) does.

## Implementation workers

Pass ordinary functions to `serve_implementation`:

```r
library(metewand)

serve_implementation(
  prepare = function(dataset_instance, problem_contract, problem_parameters,
                     implementation_parameters, seed) {
    list(values = read_json(file.path(dataset_instance, "data.json"))$values,
         offset = problem_parameters$offset)
  },
  execute = function(state, budget, observation_control) {
    state$answer <- sum(unlist(state$values)) + state$offset
    state
  },
  result = function(state, directory) {
    write_result(directory, list(answer = state$answer),
                 schema = "schemas/result.json")
  },
  statistics = function(state) list(input_count = length(state$values)),
  reset = function(state) NULL
)
```

`prepare` receives the private dataset directory and problem contract identity
as strings, two resolved parameter objects as lists, and the supplied nonnegative
53-bit implementation seed as an R number. It must establish clean state on
every call and returns the state passed to `execute`. Benchmark code chooses
how its random generators consume the seed; the SDK never calls `set.seed`.

`execute(state, budget, observation_control)` returns the state passed to the
result writer and optional statistics callback. Budget and observation control
are both `NULL` in the current one-shot protocol. State can be any R value,
including an environment or a library object; only values passed to JSON helpers
must obey the canonical restrictions below.

`result(state, directory)` writes and closes all result files, then returns a
manifest path relative to the assigned directory. The SDK replies only after
this callback returns and the manifest exists. `statistics(state)` optionally
returns a JSON object of diagnostic statistics; omission or `NULL` produces an
empty object. These diagnostics never replace independently evaluated metrics.

`implementation_time_ns` measures monotonic elapsed time immediately around
`execute`. Preparation, statistics, result writing, and protocol encoding fall
outside this diagnostic interval. An unsafe duration becomes `null`.
Executor-observed time remains authoritative and includes result writing in
every scope containing `execute`.

Execution consumes a successful preparation, including when the execution,
statistics, or result callback fails. Failed preparation or reset also leaves
no prepared execution. `reset(state)` releases slot-scoped resources before
the SDK discards the state. Without this callback, reset returns
`unsupported_operation`, so Metewand discards the process. The optional
`shutdown(state)` callback releases benchmark-owned resources before exit.
The SDK does not retain reset's or shutdown's return value.

## Dataset materializers

```r
serve_dataset_materializer(function(source_dir, dataset_parameters, seed, output_dir) {
  write_json(file.path(output_dir, "data.json"),
             list(values = dataset_parameters$values))
  write_dataset_manifest(output_dir,
    list(file_entry(output_dir, "data.json", "application/json")))
})
```

Source acquisition has already happened. The callback receives directory strings,
resolved dataset parameters, and the dataset seed. Write every dataset file under
`output_dir`, write the manifest last, close the files, and return its relative
path. The SDK does not download inputs or publish datasets.

## Independent evaluators

```r
serve_evaluator(function(dataset_instance, problem_contract, problem_parameters,
                         result_dir, metrics_path) {
  values <- read_json(file.path(dataset_instance, "data.json"))$values
  actual <- read_json(file.path(result_dir, "result.json"))$data$answer
  error <- abs(actual - (sum(unlist(values)) + problem_parameters$offset))
  write_metrics(metrics_path, list(absolute_error = error),
                schema = "schemas/metrics.json")
  if (error == 0) "reached" else "invalid"
})
```

The callback receives directory strings, the problem contract identity, resolved
problem parameters, and the exact assigned metrics path. It writes and closes
metrics before returning `invalid`, `reached`, `not_reached`, or `not_required`.
Only user-supplied evaluator code computes trusted metrics and chooses a verdict.
Metewand independently validates all outputs against the problem's schemas.

## Canonical values

`parse_json(source)` accepts UTF-8 raw bytes or a character scalar;
`read_json(path)` reads a file. `canonical_bytes(value)` returns canonical raw
UTF-8 bytes without a newline, and `write_json(path, value)` writes and closes
a canonical document. The parser rejects duplicate keys after escape decoding,
invalid UTF-8, byte-order marks, lone surrogates, nonfinite numbers, and integral
numbers outside the safe 53-bit range. Negative zero becomes zero. The encoder
uses RFC 8785 number formatting, string escaping, and UTF-16 property ordering,
independently of locale and R's display options. Nesting is bounded to 128 levels.

R values have explicit JSON meanings:

| R value | JSON value |
| --- | --- |
| `NULL` | `null` |
| Unclassed length-one logical | Boolean |
| Unclassed length-one integer or double | Number |
| Unclassed length-one character | String |
| Unnamed list, including `list()` | Array |
| Named list with unique string names | Object |
| `json_object()` | Empty object |

Atomic vectors, missing values, factors, matrices, data frames, dates, and other
classed objects are rejected. Convert them explicitly, for example with
`as.list(values)` for a numeric array. A length-one atomic vector is a scalar;
use `list(value)` for a one-element array. Every scalar must be free of
attributes. `json_object(values, keys)` can also construct objects with empty
keys, and all object keys must be unique.

R character strings cannot hold embedded NULs. To preserve every valid JSON
string, `parse_json` represents those strings as raw vectors with class
`metewand_string`; `json_string(bytes)` explicitly constructs one from valid
UTF-8 bytes. Objects containing such keys use class `metewand_object` with the
original key list in `attr(value, "keys")`. Values retain their source order
and remain accessible by numeric index; ordinary keys also support named
access. These wrappers round-trip losslessly through the JSON helpers and must
be converted explicitly before passing them to scientific libraries.

## Output helpers

| Helper | Output |
| --- | --- |
| `file_entry(directory, path, media_type, representation = NULL)` | Completed file's path, SHA-256, byte size, and explicit representation metadata. |
| `write_dataset_manifest(directory, files, manifest = "dataset-manifest.json")` | Complete dataset inventory; returns its relative path. |
| `write_result(directory, data, schema, files = list(), manifest = "result.json")` | Version-1 result envelope; returns its relative path. |
| `write_metrics(path, data, schema)` | Version-1 metrics envelope at the exact assigned path. |

Result and metrics data must be JSON objects. `files` is an unnamed list of
explicit inventory entries sorted by UTF-8 path bytes. Authors supply media
types and representation fields: `shape` as a list of nonnegative safe integers,
`data_type`, `byte_order`, and `sparse_layout`. Paths must be normalized and
relative; they cannot escape the assigned directory through symlinks.
Inventories cannot repeat paths or include their own manifest. Helpers do not
create parent directories, infer representation metadata, or validate scientific
data. Metewand's [output validator](worker-output-validation.md) verifies schemas,
complete trees, sizes, hashes, and problem contracts before publication.

## Transport and errors

Each `serve_*` function reads `METEWAND_PROTOCOL_READ_FD` and
`METEWAND_PROTOCOL_WRITE_FD`, or accepts explicit `read_fd` and `write_fd`
arguments. The descriptors must be separate binary pipes. The SDK duplicates
them and closes its own copies on return; caller-owned descriptors remain open.
Standard output and standard error remain available for logs. There is no
automatic stdin/stdout fallback.

The handshake echoes the resolved worker identity and reports `metewand-r`
version `0.1.0`. Implementations advertise only `one_shot`; materializers and
evaluators advertise no implementation capabilities. Unsupported protocol offers,
wrong roles, unknown fields, invalid seeds, and premature operations fail with
typed errors. Messages use canonical JSON Lines with the existing 1 MiB bound,
including the final newline. Malformed frames, early EOF, and missing request
IDs raise `metewand_protocol_error` and terminate serving. A failed handshake
sends one failure and returns.

Use `stop(worker_error(code, message, details = NULL))` for a structured callback
failure. Valid codes are `invalid_request`, `unsupported_operation`,
`operation_failed`, and `internal_error`. Messages must be nonempty strings, and
details must be a canonical JSON object. Other callback and output serialization
errors produce a generic `operation_failed` response and log the error to stderr.
Oversized failure payloads become a bounded `internal_error`; an ID leaving no
room for a bounded failure terminates serving.

## Compatibility and verification

This SDK uses the existing protocol, schema, canonicalization, identity, and
execution-semantics versions. It introduces no messages or fields. Applicability,
scientific budgets, and profile subprotocols remain reserved for later slices.

The R contract suite consumes the shared [canonical JSON
vectors](../fixtures/canonical-json/v1.json) and checks R value restrictions,
inventories, lifecycle, failures, and framing. Rust integration tests install the
package in an isolated temporary library and connect all three roles through
`WorkerSession`, validate artifacts, verify SDK provenance and separated logs,
and exercise reset and reuse. The CLI test runs the example through durable
publication. The development environment and CI include R and `digest`.

```sh
cargo test -p metewand-runtime --test r_sdk --locked
cargo test -p metewand-cli --test r_sdk --locked
```
