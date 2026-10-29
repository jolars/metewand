"""Serial binding callback dispatch over authenticated loopback sockets."""

import os
import re
import time
import traceback
from dataclasses import dataclass
from pathlib import Path

from ._artifacts import _object, _output_path
from ._canonical import MAX_SAFE_INTEGER, canonical_bytes, parse_json

WIRE_PROTOCOL_VERSION = 1
MAX_LINE_BYTES = 1024 * 1024
_FAILURE_CODES = {
    "invalid_request",
    "unsupported_operation",
    "operation_failed",
    "internal_error",
    "resource_limit",
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
    "check_applicability": {
        "dataset_dir": "string",
        "dataset_manifest": "string",
        "problem_contract_id": "string",
        "problem_parameters": "object",
        "implementation_parameters": "object",
    },
    "prepare": {
        "dataset_dir": "string",
        "problem_contract_id": "string",
        "problem_parameters": "object",
        "implementation_parameters": "object",
        "implementation_seed": "seed",
    },
    "execute": {"result_dir": "string"},
    "execute_buffered": {"result_root": "string", "profile": "object"},
    "execute_stream": {"result_root": "string", "profile": "object"},
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
    "implementation": {
        "check_applicability",
        "prepare",
        "execute",
        "execute_buffered",
        "execute_stream",
        "reset",
        "shutdown",
    },
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
    def __init__(self, worker, role, metadata, exchange=None):
        self.exchange = exchange
        self.worker = worker
        self.role = role
        self.ready = False
        self.prepared = False
        self.metadata = metadata
        self.semantics = role == "problem_evaluator"
        self.fresh_sequence = (
            role == "implementation" and "execute" in worker.binding_callbacks
        )
        self.applicability = (
            role == "implementation"
            and "check_applicability" in worker.binding_callbacks
        )

    def dispatch(self, request):
        if request.get("method") == "evaluate" and self.semantics:
            request = request.copy()
            semantics = request.pop("problem_semantics", None)
            if type(semantics) is not dict:
                raise WorkerError(
                    "invalid_request", "Expected problem_semantics object."
                )
        control = None
        if request.get("method") == "execute" and "observation_control" in request:
            if (
                not self.fresh_sequence
                or type(request["observation_control"]) is not dict
            ):
                raise WorkerError(
                    "invalid_request",
                    "Observation controls require fresh_sequence support and an object.",
                )
            request = request.copy()
            control = parse_json(canonical_bytes(request.pop("observation_control")))
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
            capabilities = ["managed_outputs"]
            if self.role == "implementation":
                if "execute" in self.worker.binding_callbacks:
                    capabilities.append("one_shot")
                if "execute_buffered" in self.worker.binding_callbacks:
                    capabilities.append("buffered_profile")
                if "execute_stream" in self.worker.binding_callbacks:
                    capabilities.append("streaming_profile")
                if self.applicability:
                    capabilities.append("applicability")
                if self.fresh_sequence:
                    capabilities.append("fresh_sequence")
            elif self.semantics:
                capabilities.append("problem_semantics")
            return {
                "protocol": WIRE_PROTOCOL_VERSION,
                "worker_id": request["worker_id"],
                "sdk": self.metadata,
                "capabilities": capabilities,
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
        if self.role == "implementation":
            self.worker._request_context(request)
        if method == "check_applicability":
            if not self.applicability:
                raise WorkerError(
                    "unsupported_operation", "Applicability is not implemented."
                )
            result = self.worker.check_applicability()
            _object(result, "Applicability decision")
            if set(result) - {"outcome", "code", "reason", "details"}:
                raise ValueError("Unknown applicability decision field.")
            if result.get("outcome") not in {"applicable", "not_applicable"}:
                raise ValueError("Expected applicable or not_applicable outcome.")
            for field in ("code", "reason"):
                if type(result.get(field)) is not str or not result[field]:
                    raise ValueError(f"Expected nonempty applicability {field}.")
            if "details" in result:
                _object(result["details"], "Applicability details")
            return result
        if method == "prepare":
            self.prepared = False
            self.worker.prepare()
            self.prepared = True
        elif method == "execute":
            if "execute" not in self.worker.binding_callbacks:
                raise WorkerError(
                    "unsupported_operation", "One-shot execution is not implemented."
                )
            if not self.prepared:
                raise WorkerError(
                    "invalid_request", "Execute requires a successful prepare."
                )
            self.prepared = False
            execute = self.worker.execute
            start = time.perf_counter_ns()
            output = execute(
                None, None if control is None else parse_json(canonical_bytes(control))
            )
            elapsed = time.perf_counter_ns() - start
            statistics = self.worker.statistics(output)
            statistics = (
                {}
                if statistics is None
                else _object(statistics, "Implementation statistics")
            )
            directory = Path(request["result_dir"])
            manifest = _completed_manifest(
                directory,
                self.worker.write_result(directory, output),
            )
            response = {
                "draft": manifest,
                "statistics": statistics,
                "implementation_time_ns": elapsed
                if 0 <= elapsed <= MAX_SAFE_INTEGER
                else None,
            }
            if control is not None:
                response["observation_control"] = control
            return response
        elif method == "execute_stream":
            if "execute_stream" not in self.worker.binding_callbacks:
                raise WorkerError("unsupported_operation", "Streaming execution is not implemented.")
            if not self.prepared:
                raise WorkerError("invalid_request", "Streaming execution requires preparation.")
            self.prepared = False
            from ._streaming import StreamingContext
            stream = StreamingContext(request["profile"], Path(request["result_root"]), request["id"], self.exchange)
            self.worker.execute_stream(stream)
            return {"checkpoint_count": stream.count}
        elif method == "execute_buffered":
            if "execute_buffered" not in self.worker.binding_callbacks:
                raise WorkerError(
                    "unsupported_operation", "Buffered execution is not implemented."
                )
            if not self.prepared:
                raise WorkerError(
                    "invalid_request", "Buffered execution requires preparation."
                )
            self.prepared = False
            from ._buffered import BufferedTrace

            trace = BufferedTrace(request["profile"], Path(request["result_root"]))
            self.worker.execute_buffered(None, trace)
            return {
                "buffer": {
                    "manifest": _completed_manifest(trace.directory, trace.write())
                }
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
            return {"draft": _completed_manifest(directory, manifest)}
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
            from ._bindings import _draft

            manifest = _draft(metrics_path.parent, "metrics", verdict.metrics)
            return {"verdict": verdict.verdict, "draft": manifest}
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


def _serve(worker, role, reader, writer, *, metadata, initialize=None):
    def exchange(checkpoint):
        writer.write(_encode_response(checkpoint))
        writer.flush()
        return _read_frame(reader)
    dispatcher = _Dispatcher(worker, role, metadata, exchange)
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
            if request["method"] == "hello" and initialize is not None:
                initialize()
            response = {"id": request_id, "ok": True, **payload}
            frame = _encode_response(response)
            success = True
        except WorkerError as error:
            frame = _encode_failure(request_id, error)
        except Exception:  # noqa: BLE001 - Callbacks may raise arbitrary exceptions.
            # Failed attempts persist wire diagnostics even when stderr is unavailable.
            text = (
                traceback.format_exc()
                .encode("utf-8", "backslashreplace")
                .decode("utf-8")
            )
            if len(text) > 8192:
                text = text[:4096] + "\n[traceback truncated]\n" + text[-4096:]
            traceback.print_exc()
            frame = _encode_failure(
                request_id,
                WorkerError(
                    "operation_failed",
                    "Worker callback or output serialization failed:\n" + text,
                ),
            )
        writer.write(frame)
        writer.flush()
        if not dispatcher.ready or (request.get("method") == "shutdown" and success):
            return


def _serve_socket(worker, role, *, metadata, initialize=None):
    import socket

    with socket.create_connection(
        ("127.0.0.1", int(os.environ["METEWAND_PROTOCOL_PORT"]))
    ) as connection:
        connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        connection.sendall(
            os.environ["METEWAND_PROTOCOL_TOKEN"].encode("ascii") + b"\n"
        )
        with connection.makefile("rb") as reader, connection.makefile("wb") as writer:
            _serve(
                worker, role, reader, writer, metadata=metadata, initialize=initialize
            )
