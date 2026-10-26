"""Serial callback dispatch over the inherited version-1 protocol pipes."""

import os
import re
import time
import traceback
from abc import ABC, abstractmethod
from contextlib import ExitStack
from dataclasses import dataclass
from pathlib import Path

from ._artifacts import _object, _output_path, write_metrics, write_result
from ._canonical import MAX_SAFE_INTEGER, canonical_bytes, parse_json

SDK_VERSION = "0.1.0"
WIRE_PROTOCOL_VERSION = 1
MAX_LINE_BYTES = 1024 * 1024
_FAILURE_CODES = {
    "invalid_request",
    "unsupported_operation",
    "operation_failed",
    "internal_error",
}
_VERDICTS = {"invalid", "reached", "not_reached", "not_required"}


class ProtocolError(ValueError):
    """A malformed frame or envelope prevents further protocol dispatch."""


class WorkerError(Exception):
    """A callback failure with a stable wire code and optional JSON object details."""

    def __init__(self, code, message, details=None):
        if code not in _FAILURE_CODES:
            raise ValueError("Unknown version-1 worker failure code.")
        if type(message) is not str or not message:
            raise ValueError("Worker failure messages must be nonempty strings.")
        canonical_bytes(message)
        if details is not None:
            _object(details, "Worker failure details")
        super().__init__(message)
        self.code = code
        self.details = details


class _ImplementationLifecycle(ABC):
    """Shared lifecycle for data callbacks and custom artifact writers."""

    @abstractmethod
    def prepare(
        self,
        dataset_instance,
        problem_contract,
        problem_parameters,
        implementation_parameters,
        seed,
    ):
        """Establish clean state from the private dataset Path and supplied values."""

    @abstractmethod
    def execute(self, budget, observation_control):
        """Run the computation and return diagnostic statistics or None.

        Both arguments are None in the current one-shot protocol.
        """

    def reset(self):
        """Override to clear slot-scoped state when process reuse is appropriate."""
        raise WorkerError(
            "unsupported_operation", "This implementation does not support reset."
        )

    def shutdown(self):
        """Release benchmark-owned resources before the process exits."""


class Implementation(_ImplementationLifecycle):
    """An implementation with diagnostic execute output and a custom writer."""

    @abstractmethod
    def write_result(self, directory):
        """Close all result files and return a manifest path relative to directory."""


class OneShotImplementation(_ImplementationLifecycle):
    """Return scientific data from execute, with optional conversion afterward.

    prepare, execute, reset, and shutdown have the same arguments as Implementation.
    execute returns the value passed to statistics and write_result. Only execute
    is included in the SDK's diagnostic implementation time.
    """

    @abstractmethod
    def execute(self, budget, observation_control):
        """Run the computation and return scientific data or a solver-owned value."""

    def result(self, output):
        """Convert solver output to a JSON object outside diagnostic solve timing."""
        return output

    def statistics(self, output):
        """Return optional diagnostic statistics outside diagnostic solve timing."""
        return None

    def write_result(self, directory, output):
        """Write the standard envelope, or override for custom artifact files."""
        return write_result(directory, self.result(output))


@dataclass(frozen=True)
class Evaluation:
    """Evaluator-owned metric data and an explicit version-1 scientific verdict."""

    metrics: dict
    verdict: str

    def __post_init__(self):
        _object(self.metrics, "Metrics data")
        if type(self.verdict) is not str or self.verdict not in _VERDICTS:
            raise ValueError("Evaluators must return a version-1 scientific verdict.")


_FIELDS = {
    "hello": {"protocols": "protocols", "role": "role", "worker_id": "worker_id"},
    "materialize": {
        "source_dir": "string",
        "dataset_parameters": "object",
        "dataset_seed": "seed",
        "output_dir": "string",
    },
    "prepare": {
        "dataset_dir": "string",
        "problem_contract_id": "string",
        "problem_parameters": "object",
        "implementation_parameters": "object",
        "implementation_seed": "seed",
    },
    "execute": {"result_dir": "string"},
    "reset": {},
    "evaluate": {
        "dataset_dir": "string",
        "problem_contract_id": "string",
        "problem_parameters": "object",
        "result_dir": "string",
        "metrics_path": "string",
    },
    "shutdown": {},
}
_METHODS = {
    "implementation": {"prepare", "execute", "reset", "shutdown"},
    "dataset_materializer": {"materialize", "shutdown"},
    "problem_evaluator": {"evaluate", "shutdown"},
}


def _validate_request(request):
    if (
        type(request) is not dict
        or type(request.get("id")) is not str
        or not request["id"]
    ):
        # A fabricated ID would make this response impossible to correlate.
        raise ProtocolError("A request must be an object with a nonempty string ID.")
    method = request.get("method")
    if type(method) is not str or not method:
        raise WorkerError("invalid_request", "A request must name a method.")
    if method not in _FIELDS:
        raise WorkerError("unsupported_operation", f"Unsupported method: {method}.")
    fields = _FIELDS[method]
    if set(request) != {"id", "method"} | set(fields):
        raise WorkerError(
            "invalid_request", f"Unexpected or missing fields for {method}."
        )
    for key, kind in fields.items():
        value = request[key]
        valid = False
        if kind == "string":
            valid = type(value) is str and bool(value)
        elif kind == "object":
            valid = type(value) is dict
        elif kind == "seed":
            valid = type(value) is int and 0 <= value <= MAX_SAFE_INTEGER
        elif kind == "role":
            valid = type(value) is str and value in _METHODS
        elif kind == "worker_id":
            valid = type(value) is str and bool(
                re.fullmatch("mw1-resolved-worker-[0-9a-f]{64}", value)
            )
        elif kind == "protocols":
            valid = (
                type(value) is list
                and bool(value)
                and all(
                    type(version) is int and 0 <= version <= (1 << 32) - 1
                    for version in value
                )
                and len(set(value)) == len(value)
            )
        if not valid:
            raise WorkerError("invalid_request", f"Invalid {key} for {method}.")
    return method


def _read_frame(reader):
    line = reader.readline(MAX_LINE_BYTES + 1)
    if not line:
        raise ProtocolError("Protocol stream ended before shutdown.")
    if len(line) > MAX_LINE_BYTES:
        raise ProtocolError("Protocol line exceeds the 1 MiB limit.")
    if not line.endswith(b"\n"):
        raise ProtocolError("Protocol line ended without a newline.")
    try:
        return parse_json(line)
    except ValueError as error:
        raise ProtocolError(str(error)) from error


def _encode_response(response):
    frame = canonical_bytes(response) + b"\n"
    if len(frame) > MAX_LINE_BYTES:
        raise ValueError("Response exceeds the 1 MiB protocol line limit.")
    return frame


def _completed_manifest(directory, manifest):
    if not isinstance(manifest, (str, Path)):
        raise TypeError("The callback must return a relative manifest path.")
    target = _output_path(directory, manifest)
    if not target.is_file():
        raise ValueError("The callback returned before writing its manifest.")
    return str(manifest)


class _Dispatcher:
    def __init__(self, worker, role, semantics=False):
        self.worker = worker
        self.role = role
        self.ready = False
        self.prepared = False
        self.semantics = semantics

    def dispatch(self, request):
        if request.get("method") == "evaluate" and self.semantics:
            request = request.copy()
            semantics = request.pop("problem_semantics", None)
            if type(semantics) is not dict:
                raise WorkerError(
                    "invalid_request", "Expected problem_semantics object."
                )
        method = _validate_request(request)
        if not self.ready:
            if method != "hello" or request["role"] != self.role:
                raise WorkerError(
                    "invalid_request",
                    "The first request must negotiate this worker's role.",
                )
            if WIRE_PROTOCOL_VERSION not in request["protocols"]:
                raise WorkerError(
                    "unsupported_operation", "No offered protocol version is supported."
                )
            self.ready = True
            return {
                "protocol": WIRE_PROTOCOL_VERSION,
                "worker_id": request["worker_id"],
                "sdk": {"name": "metewand-python", "version": SDK_VERSION},
                "capabilities": ["one_shot"]
                if self.role == "implementation"
                else (["problem_semantics"] if self.semantics else []),
            }
        if method == "hello":
            raise WorkerError(
                "invalid_request",
                "The worker session has already negotiated a protocol.",
            )
        if method not in _METHODS[self.role]:
            raise WorkerError(
                "unsupported_operation",
                f"The {self.role} role does not accept {method}.",
            )
        if method == "prepare":
            self.prepared = False
            self.worker.prepare(
                Path(request["dataset_dir"]),
                request["problem_contract_id"],
                request["problem_parameters"],
                request["implementation_parameters"],
                request["implementation_seed"],
            )
            self.prepared = True
        elif method == "execute":
            if not self.prepared:
                raise WorkerError(
                    "invalid_request", "Execute requires a successful prepare."
                )
            self.prepared = False
            execute = self.worker.execute
            start = time.perf_counter_ns()
            output = execute(None, None)
            elapsed = time.perf_counter_ns() - start
            data_callback = isinstance(self.worker, OneShotImplementation)
            statistics = self.worker.statistics(output) if data_callback else output
            statistics = (
                {}
                if statistics is None
                else _object(statistics, "Implementation statistics")
            )
            directory = Path(request["result_dir"])
            manifest = _completed_manifest(
                directory,
                self.worker.write_result(directory, output)
                if data_callback else self.worker.write_result(directory),
            )
            return {
                "result": {"manifest": manifest},
                "statistics": statistics,
                "implementation_time_ns": elapsed
                if 0 <= elapsed <= MAX_SAFE_INTEGER
                else None,
            }
        elif method == "reset":
            self.prepared = False
            self.worker.reset()
        elif method == "materialize":
            directory = Path(request["output_dir"])
            manifest = self.worker(
                Path(request["source_dir"]),
                request["dataset_parameters"],
                request["dataset_seed"],
                directory,
            )
            return {"dataset": {"manifest": _completed_manifest(directory, manifest)}}
        elif method == "evaluate":
            metrics_path = Path(request["metrics_path"])
            verdict = self.worker(
                Path(request["dataset_dir"]),
                request["problem_contract_id"],
                request["problem_parameters"],
                *([semantics] if self.semantics else []),
                Path(request["result_dir"]),
                metrics_path,
            )
            if isinstance(verdict, Evaluation):
                write_metrics(metrics_path, verdict.metrics)
                verdict = verdict.verdict
            if type(verdict) is not str or verdict not in _VERDICTS:
                raise ValueError(
                    "Evaluators must return a version-1 scientific verdict."
                )
            if not metrics_path.is_file():
                raise ValueError("The evaluator returned before writing metrics.")
            return {"verdict": verdict}
        elif method == "shutdown" and self.role == "implementation":
            self.worker.shutdown()
        return {}


def _failure(request_id, error):
    payload = {"code": error.code, "message": str(error)}
    if error.details is not None:
        payload["details"] = error.details
    return {"id": request_id, "ok": False, "error": payload}


def _encode_failure(request_id, error):
    try:
        return _encode_response(_failure(request_id, error))
    except ValueError:
        try:
            return _encode_response(
                _failure(
                    request_id,
                    WorkerError(
                        "internal_error",
                        "Worker failure could not be encoded within the protocol limit.",
                    ),
                )
            )
        except ValueError as failure:
            raise ProtocolError(
                "Request ID leaves no room for a bounded failure response."
            ) from failure


def _serve(worker, role, reader, writer, semantics=False):
    dispatcher = _Dispatcher(worker, role, semantics=semantics)
    while True:
        request = _read_frame(reader)
        # Check the envelope before attempting to build any failure response.
        if (
            type(request) is not dict
            or type(request.get("id")) is not str
            or not request["id"]
        ):
            raise ProtocolError(
                "A request must be an object with a nonempty string ID."
            )
        request_id = request["id"]
        success = False
        try:
            payload = dispatcher.dispatch(request)
            response = {"id": request_id, "ok": True, **payload}
            frame = _encode_response(response)
            success = True
        except WorkerError as error:
            frame = _encode_failure(request_id, error)
        except Exception:  # noqa: BLE001 - Callbacks may raise arbitrary exceptions.
            # Tracebacks belong in the separately drained stderr log. A bounded
            # generic wire message also accommodates exceptions with invalid text.
            traceback.print_exc()
            frame = _encode_failure(
                request_id,
                WorkerError(
                    "operation_failed",
                    "Worker callback or output serialization failed; see the worker log.",
                ),
            )
        writer.write(frame)
        writer.flush()
        if not dispatcher.ready or (request.get("method") == "shutdown" and success):
            return


def _serve_fds(worker, role, read_fd, write_fd, semantics=False):
    if read_fd is None:
        read_fd = int(os.environ["METEWAND_PROTOCOL_READ_FD"])
    if write_fd is None:
        write_fd = int(os.environ["METEWAND_PROTOCOL_WRITE_FD"])
    if read_fd == write_fd:
        raise ValueError("The protocol requires separate read and write descriptors.")
    with ExitStack() as stack:
        reader = stack.enter_context(os.fdopen(os.dup(read_fd), "rb"))
        writer = stack.enter_context(os.fdopen(os.dup(write_fd), "wb"))
        _serve(worker, role, reader, writer, semantics=semantics)


def serve_implementation(implementation, *, read_fd=None, write_fd=None):
    """Serve a one-shot Implementation on inherited or explicitly supplied pipes."""
    if not isinstance(implementation, (Implementation, OneShotImplementation)):
        raise TypeError("Expected an Implementation or OneShotImplementation instance.")
    _serve_fds(implementation, "implementation", read_fd, write_fd)


def serve_dataset_materializer(materialize, *, read_fd=None, write_fd=None):
    """Serve materialize(source_dir, dataset_parameters, seed, output_dir).

    The callback writes its files and returns a relative dataset manifest path.
    """
    if not callable(materialize):
        raise TypeError("Expected a materializer callback.")
    _serve_fds(materialize, "dataset_materializer", read_fd, write_fd)


def serve_evaluator(evaluate, *, semantics=False, read_fd=None, write_fd=None):
    """Serve evaluate(dataset, contract, parameters, result_dir, metrics_path).

    Return Evaluation(metrics, verdict) to write the standard envelope, or write
    metrics directly and return invalid, reached, not_reached, or not_required.
    Only benchmark-owned evaluator code supplies this verdict.
    With semantics=True, declared scientific rules follow parameters in the callback.
    """
    if not callable(evaluate):
        raise TypeError("Expected an evaluator callback.")
    if type(semantics) is not bool:
        raise TypeError("semantics must be a boolean.")
    _serve_fds(evaluate, "problem_evaluator", read_fd, write_fd, semantics=semantics)
