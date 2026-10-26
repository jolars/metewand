# Python worker helpers

The [Python SDK](https://github.com/jolars/metewand/tree/main/sdk/python) supports the existing version-1 one-shot worker
protocol. It requires Python 3.11 or later and uses only the standard library at
runtime. Install it into the worker's selected environment, or include its
`metewand` package in a declared source bundle as the [runnable
example](https://github.com/jolars/metewand/tree/main/examples/python-sdk) does. An ambient `PYTHONPATH` is not inherited
by the trusted local launcher.

## Implementation workers

Subclass `OneShotImplementation` and pass an instance to `serve_implementation`:

```python
from metewand import OneShotImplementation, read_json, serve_implementation


class Sum(OneShotImplementation):
    def prepare(self, dataset_instance, problem_contract, problem_parameters,
                implementation_parameters, seed):
        self.values = read_json(dataset_instance / "data.json")["values"]
        self.offset = problem_parameters["offset"]

    def execute(self, budget, observation_control):
        return {"answer": sum(self.values) + self.offset}

    def statistics(self, output):
        return {"input_count": len(self.values)}

    def reset(self):
        self.__dict__.clear()


serve_implementation(Sum())
```

`prepare` receives a `pathlib.Path` to the private dataset instance, the problem
contract identity string, two resolved parameter dictionaries, and the supplied
nonnegative 53-bit implementation seed. Each call must establish clean state.
Benchmark code chooses how its own random generators consume the supplied seed.

`execute` receives `None` for both budget and observation control in the current
one-shot protocol. It returns scientific result data as a JSON object. The SDK
writes `result.json` with the standard version-1 envelope and an empty file
inventory. For a solver that returns its own object, override `result(output)`
to convert it to scientific JSON data. This conversion and the optional
`statistics(output)` callback run after diagnostic solve timing. Statistics
must be a JSON object or `None`; they never replace independent metrics.

Override `write_result(directory, output)` to write custom artifact files and
return a completed manifest path relative to the assigned `Path`. The existing
`Implementation` class also remains available: its `execute` returns diagnostic
statistics, and its required `write_result(directory)` writes benchmark-owned
results stored on the instance. Existing callbacks keep their original meaning.

The SDK replies only after the result manifest exists. Callbacks must close all
files before returning. An execution requires a successful preparation and
consumes that prepared state, including when execution fails.

`write_result` and `write_metrics` may omit `schema` to use the selected problem
contract's schema. This supports inline schemas without duplicating their
declarations in worker code. An explicitly supplied schema path remains checked.

The SDK measures `implementation_time_ns` with `time.perf_counter_ns`
immediately around `execute`. Preparation, statistics validation, result
writing, and protocol serialization are outside that diagnostic interval. A
duration outside the version-1 safe integer range becomes `null`.
Executor-observed timing remains authoritative and includes result serialization
in every scope containing `execute`.

Override `reset` when process reuse is appropriate. Its default returns
`unsupported_operation`, which causes Metewand to discard the worker rather than
assume its state is clean. A failed preparation or reset callback, or a failed
execution, leaves no prepared execution available. The optional `shutdown`
method releases benchmark-owned resources; its default does nothing.
Materializer and evaluator callbacks have no process reuse state API.

## Dataset materializers

Pass a function to `serve_dataset_materializer`:

```python
from metewand import (
    file_entry, serve_dataset_materializer, write_dataset_manifest, write_json,
)


def materialize(source_dir, dataset_parameters, seed, output_dir):
    write_json(output_dir / "data.json", {"values": dataset_parameters["values"]})
    files = [file_entry(output_dir, "data.json", "application/json")]
    return write_dataset_manifest(output_dir, files)


serve_dataset_materializer(materialize)
```

The callback receives the assigned source and output directories as `Path`
objects, the resolved dataset parameter dictionary, and the dataset seed. Source
acquisition has already happened. Write every dataset file beneath `output_dir`,
then write the manifest last and return its relative path. The SDK does not
download inputs or publish dataset instances.

## Independent evaluators

Pass a problem-owned evaluation function to `serve_evaluator`:

```python
from metewand import Evaluation, read_json, serve_evaluator


def evaluate(dataset_instance, problem_contract, problem_parameters,
             problem_semantics, result_dir, metrics_path):
    values = read_json(dataset_instance / "data.json")["values"]
    expected = sum(values) + problem_parameters["offset"]
    actual = read_json(result_dir / "result.json")["data"]["answer"]
    error = abs(actual - expected)
    target = problem_semantics["one_shot_completion"]["absolute_error"]
    return Evaluation({"absolute_error": error},
                      "reached" if error <= target else "not_reached")


serve_evaluator(evaluate, semantics=True)
```

The callback receives dataset and result directories as `Path` objects, the
problem contract identity, the resolved problem parameter dictionary, the
canonical scientific rules, and the exact assigned metrics path. Return
`Evaluation(metrics, verdict)` to have the SDK write and close the standard
metrics envelope at that path. Metrics must be a canonical JSON object, and the
verdict must be `invalid`, `reached`, `not_reached`, or `not_required`. A custom
callback can still write metrics itself and return the verdict string. The SDK
does not compute metrics or infer a verdict. Metewand independently validates
the result and metrics against the problem's schemas and applies the observation
policy.

With `semantics=True`, the SDK advertises `problem_semantics` and requires its
object in evaluation requests. The default `semantics=False` preserves the
five-argument callback and original wire shape for existing evaluators.

## JSON and output helpers

`parse_json(bytes_or_text)` and `read_json(path)` reject duplicate object keys,
invalid UTF-8, byte-order marks, lone Unicode surrogates, nonfinite numbers, and
numerically integral values outside the safe 53-bit range. They normalize
integral floats and negative zero to integers. `canonical_bytes(value)` and
`write_json(path, value)` emit the exact version-1 RFC 8785 representation.
Accepted Python values are built-in `None`, `bool`, `int`, `float`, `str`,
`list`, and `dict` with string keys. Convert library arrays and scalars
explicitly; tuples, nonstring keys, and custom objects are not coerced.

The output helpers use existing version-1 envelopes:

  | Helper                                                                        | Output                                                                                                |
  | ----------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
  | `file_entry(directory, path, media_type, representation=None)`                | An inventory entry with the completed file's SHA-256 and byte size. Pass `representation` by keyword. |
  | `write_dataset_manifest(directory, files, manifest="dataset-manifest.json")`  | A complete artifact manifest and its relative path. Pass `manifest` by keyword.                       |
  | `write_result(directory, data, schema=None, files=(), manifest="result.json")` | A result envelope and its relative path. Pass `schema`, `files`, and `manifest` by keyword.           |
  | `write_metrics(path, data, schema=None)`                                       | A metrics envelope at the exact assigned path. Pass `schema` by keyword.                              |

The author supplies file media types and any representation metadata: `shape`,
`data_type`, `byte_order`, and `sparse_layout`. Inventories use explicit file
entries sorted by UTF-8 path bytes. Paths must be normalized and relative;
inventories cannot contain duplicate paths or their own manifest. Helpers do not
create parent directories or infer schemas and representations.

These helpers validate the JSON domain and envelope metadata. They do not
validate scientific data against a problem schema, verify a complete output
tree, or publish artifacts. Metewand performs those checks using its existing
[worker-output validator](worker-output-validation.md).

## Transport and errors

Each `serve_*` function reads `METEWAND_PROTOCOL_READ_FD` and
`METEWAND_PROTOCOL_WRITE_FD`, or accepts explicit `read_fd` and `write_fd`
keyword arguments. The descriptors must identify separate binary pipes. The SDK
closes its own duplicate descriptors when serving ends. Standard output and
standard error remain available for logs; there is no automatic stdin/stdout
fallback.

The handshake echoes the supplied resolved worker identity and reports
`metewand-python` version `0.1.0`. Implementations advertise only `one_shot`;
materializers and evaluators advertise no implementation capabilities. The SDK
rejects unsupported protocol offers, wrong roles, unknown fields, invalid seeds,
and operations before negotiation. Messages are canonical JSON Lines with the
existing 1 MiB limit, including the final newline. Malformed frames and missing
request IDs raise `ProtocolError` and terminate serving because the SDK cannot
safely correlate a response. A failed handshake sends one failure and returns.

Callbacks may raise `WorkerError(code, message, details=None)` using
`invalid_request`, `unsupported_operation`, `operation_failed`, or
`internal_error`. Messages must be nonempty, and optional details must be a
canonical JSON object. The SDK preserves these values in the failure response.
Other callback or output serialization exceptions produce `operation_failed` and
include a traceback in the failure message as well as the worker's stderr log.
The response retains the first and last 4,096 characters, with a truncation
marker when necessary; invalid Unicode is escaped. Failed attempt diagnostics
persist this message. Use `status --output-dir RUN --output json` to find the
`diagnostic_id`, then `explain ID --output-dir RUN` to inspect it after execution.
Failure payloads exceeding the
line limit become a bounded `internal_error`; an ID that itself prevents a
bounded response terminates serving. Metewand combines the failure code with the
active execution phase.

## Compatibility and verification

The wire, public-schema, canonicalization, identity, and execution-semantics
versions are unchanged. The optional `problem_semantics` request field is
guarded by the evaluator capability of the same name. Applicability, scientific
budgets, and profile subprotocols remain reserved for later roadmap slices.

The Python tests consume shared canonicalization, identity, seed, error, and
protocol vectors, check lifecycle dispatch, callback errors, malformed frames,
and exact response bytes. Deterministic timing tests exclude statistics
validation and result serialization from `implementation_time_ns` and reopen
completed results before replying. See [SDK conformance](sdk-conformance.md) for
the vectors and reference problem suites. Rust integration tests run
that suite and connect all three Python roles through `WorkerSession`, validate
their files and schemas, check SDK metadata and separated logs, and exercise
reset and reuse. The CLI test runs the example through durable publication. Run
all of these checks with the normal workspace test command, or run just the SDK
checks with:

```sh
PYTHONPATH=sdk/python/src python3 -m unittest discover -s sdk/python/tests -v
cargo test -p metewand-runtime --test python_sdk --locked
cargo test -p metewand-cli --test python_sdk --locked
```
