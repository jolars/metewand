# R worker helpers

The [R package](https://github.com/jolars/metewand/tree/main/sdk/r) implements the existing version-1 one-shot protocol
for implementations, dataset materializers, and independent evaluators. It
requires R 4.2 or later, POSIX, `digest`, and a C++17 compiler with floating-point
`std::to_chars` support. The native code handles canonical JSON, dedicated pipes,
and monotonic timing. Each worker remains a separate process.

Install the package into the selected worker environment with
`R CMD INSTALL sdk/r`. The local launcher assigns a private `R_LIBS_USER` and
does not inherit ambient language search paths. Install dependencies into the
selected environment's site library, or explicitly bundle an installed SDK as
the [runnable example](https://github.com/jolars/metewand/tree/main/examples/r-sdk) does.

## Function workers

Declare `function: benchmark.R:solve` in YAML. The source file only defines
ordinary functions; the manifest launches `metewand::serve_function()`:

```r
generate <- function(context) {
  set.seed(context$seed %% .Machine$integer.max)
  list(values = as.list(sample.int(10L, context$parameters$size, replace = TRUE) - 1L))
}

solve <- function(data, context) {
  list(total = sum(unlist(data$values)))
}

evaluate <- function(data, result, context) {
  list(absolute_error = abs(result$total - sum(unlist(data$values))))
}
```

The context is a list with matching fields across Python and R:

| Field | Dataset generator | Implementation | Evaluator |
| --- | --- | --- | --- |
| `parameters` | Dataset parameters | Implementation parameters | Problem parameters |
| `problem_parameters` | Empty JSON object | Problem parameters | Problem parameters |
| `seed` | Derived dataset seed | Derived implementation seed | `NULL` |
| `semantics` | Empty JSON object | Empty JSON object | Resolved scientific rules |

Callbacks choose how to seed R's random generator. The SDK sources the file
during the worker handshake, outside the default `prepare_and_execute`
interval. `cold_end_to_end` includes startup and sourcing. Static `check` and
`plan` never source it. Sibling files sourced during loading resolve relative to
that file's directory and must be included in the declared `sources`.

Callbacks return canonical JSON objects as named lists. Use `json_object()` for
an empty object and unnamed lists for JSON arrays. Generated data becomes
`data.json`; the SDK writes standard result and metric envelopes. Dataset-free
problems pass an empty JSON object as data. Install the SDK and dependencies
in the selected R environment's site library; workers do not inherit an ambient
user library.

Without validity or completion rules, plain metrics imply `not_required`.
Declared rules require `evaluation(metrics, verdict)`. Read targets from YAML:

```r
evaluate <- function(data, result, context) {
  error <- abs(result$total - sum(unlist(data$values)))
  target <- context$semantics$one_shot_completion$absolute_error
  metewand::evaluation(list(absolute_error = error),
                      if (error <= target) "reached" else "not_reached")
}
```

The helper rejects completion verdicts without completion rules and
`not_required` when a completion rule exists. Use `invalid` for unusable results.
Metric fields can be optional when invalid results cannot produce them. The
scientific interpretation of rules remains the evaluator's responsibility.

The initial function interface supports one-shot execution and JSON objects.
Use lifecycle callbacks below for separate preparation, binary artifacts,
profiles, and other stateful requirements. `serve_function(role, file, name)`
can also select a function explicitly; by default it reads the three launcher
arguments. Benchmark files need no serving call.

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
    list(answer = sum(unlist(state$values)) + state$offset,
         input_count = length(state$values))
  },
  result_data = function(output) list(answer = output$answer),
  statistics = function(output) list(input_count = output$input_count),
  reset = function(state) NULL
)
```

`prepare` receives the private dataset directory and problem contract identity
as strings, two resolved parameter objects as lists, and the supplied nonnegative
53-bit implementation seed as an R number. It must establish clean state on
every call and returns the state passed to `execute`. Benchmark code chooses
how its random generators consume the seed; the SDK never calls `set.seed`.

`execute(state, budget, observation_control)` returns scientific result data or
the state passed to conversion and statistics callbacks. Budget and observation control
are both `NULL` in the current one-shot protocol. State can be any R value,
including an environment or a library object; only values passed to JSON helpers
must obey the canonical restrictions below.

Omit `result` to have the SDK write `result.json` with the standard envelope and
an empty file inventory. Without `result_data`, the execute output must be a JSON
object. For a solver that returns its own state, supply `result_data(state)` to
convert it to a scientific JSON object after diagnostic solve timing.

The optional `result(state, directory)` remains the custom artifact writer. It
writes and closes all result files, then returns a manifest path relative to
the assigned directory. Supply either `result` or `result_data`. The SDK replies
only after the manifest exists. `statistics(state)` optionally
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
                         problem_semantics, result_dir, metrics_path) {
  values <- read_json(file.path(dataset_instance, "data.json"))$values
  actual <- read_json(file.path(result_dir, "result.json"))$data$answer
  error <- abs(actual - (sum(unlist(values)) + problem_parameters$offset))
  target <- problem_semantics$one_shot_completion$absolute_error
  evaluation(list(absolute_error = error),
             if (error <= target) "reached" else "not_reached")
}, semantics = TRUE)
```

The callback receives directory strings, the problem contract identity, resolved
problem parameters, canonical scientific rules, and the exact assigned metrics
path. Return `evaluation(metrics, verdict)` to have the SDK write and close the
standard metrics envelope. Metrics must be a canonical JSON object; the verdict
must be `invalid`, `reached`, `not_reached`, or `not_required`. A custom evaluator
can still write metrics itself and return the verdict string.
Only user-supplied evaluator code computes trusted metrics and chooses a verdict.
Metewand independently validates all outputs against the problem's schemas.

With `semantics = TRUE`, the SDK advertises `problem_semantics` and requires
the rules object in evaluation requests. The default `FALSE` preserves the
five-argument callback and original wire shape for existing evaluators.

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
| `write_result(directory, data, schema = NULL, files = list(), manifest = "result.json")` | Version-1 result envelope; returns its relative path. |
| `write_metrics(path, data, schema = NULL)` | Version-1 metrics envelope at the exact assigned path. |

Omit `schema` to use the selected problem contract's schema, including an inline
declaration. An explicitly supplied schema path remains checked.

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
errors produce an `operation_failed` response containing the condition message
and log it to stderr. The response retains at most 8,192 condition characters
plus a truncation marker. Failed attempt diagnostics persist the message; find
the `diagnostic_id` with `status --output-dir RUN --output json`, then inspect
it with `explain ID --output-dir RUN`.
Oversized failure payloads become a bounded `internal_error`; an ID leaving no
room for a bounded failure terminates serving.

## Compatibility and verification

This SDK uses the existing protocol, schema, canonicalization, identity, and
execution-semantics versions. The optional `problem_semantics` request field is
guarded by the evaluator capability of the same name. Applicability, scientific
budgets, and profile subprotocols remain reserved for later slices.

The R contract suite consumes shared canonicalization, identity, seed, error,
and protocol vectors and checks R value restrictions, inventories, lifecycle,
failures, and framing. Deterministic timing tests exclude returned-state storage,
statistics, and result writing from `implementation_time_ns` and reopen completed
outputs before replying. See [SDK conformance](sdk-conformance.md) for the vectors
and reference problem suites. Rust integration tests install the
package in an isolated temporary library and connect all three roles through
`WorkerSession`, validate artifacts, verify SDK provenance and separated logs,
and exercise reset and reuse. The CLI test runs the example through durable
publication. The development environment and CI include R and `digest`.

```sh
cargo test -p metewand-runtime --test r_sdk --locked
cargo test -p metewand-cli --test r_sdk --locked
```

## Fresh-sequence controls

Opt in with `serve_implementation(..., fresh_sequence=TRUE)` to report
`fresh_sequence`. The existing execute callback receives the selected
`observation_control`; the scientific budget remains absent. The SDK forwards
the control and preserves its response echo before user code can mutate it.
Existing result conversion, serialization, reset, and shutdown callbacks apply.
See [Fresh-sequence profiles](fresh-sequence-profiles.md) for a complete example.
