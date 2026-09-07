"""Direct version-1 worker fixtures with no Metewand SDK dependency."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import sys
import time
from typing import BinaryIO, NoReturn


PROTOCOL_READ_FD_ENV = "METEWAND_PROTOCOL_READ_FD"
PROTOCOL_WRITE_FD_ENV = "METEWAND_PROTOCOL_WRITE_FD"
MAX_LINE_BYTES = 1024 * 1024


class RequestError(Exception):
    """A request is not valid for the selected fixture role or state."""


def run_role(role: str) -> int:
    """Run one successful raw fixture role until shutdown."""

    reader, writer = _protocol_streams()
    try:
        _accept_hello(reader, writer, role)
        state: dict[str, object] | None = None

        while True:
            request = _read_request(reader)
            request_id = request.get("id")
            if not isinstance(request_id, str) or not request_id:
                raise RequestError("requests require a nonempty string ID")

            method = request.get("method")
            if method == "shutdown":
                _require_shape(request, "shutdown", set())
                _send(writer, {"id": request_id, "ok": True})
                return 0

            try:
                if role == "dataset_materializer" and method == "materialize":
                    response = _materialize(request)
                elif role == "implementation" and method == "prepare":
                    state = _prepare(request)
                    response = {"id": request_id, "ok": True}
                elif role == "implementation" and method == "execute":
                    response = _execute(request, state)
                elif role == "implementation" and method == "reset":
                    _require_shape(request, "reset", set())
                    state = None
                    response = {"id": request_id, "ok": True}
                elif role == "problem_evaluator" and method == "evaluate":
                    response = _evaluate(request)
                else:
                    raise RequestError(f"{role} does not support {method!r}")
            except (KeyError, OSError, RequestError, TypeError, ValueError) as error:
                response = {
                    "error": {
                        "code": "operation_failed",
                        "details": {"fixture": role},
                        "message": str(error) or "fixture operation failed",
                    },
                    "id": request_id,
                    "ok": False,
                }
            _send(writer, response)
    finally:
        reader.close()
        writer.close()


def run_protocol_failure(mode: str) -> int:
    """Emit one named framing or request-correlation fixture response."""

    reader, writer = _protocol_streams()
    try:
        hello = _read_request(reader)
        valid_response = _hello_response(hello)
        valid_bytes = _json_bytes(valid_response)

        if mode == "fragmented_reads":
            _write_fragmented(writer, valid_bytes)
            shutdown = _read_request(reader)
            _require_shape(shutdown, "shutdown", set())
            _write_fragmented(
                writer,
                _json_bytes({"id": shutdown["id"], "ok": True}),
            )
        elif mode == "duplicate_keys":
            _write_raw(
                writer,
                valid_bytes.replace(b'{"capabilities"', b'{"id":"0","capabilities"'),
            )
        elif mode == "invalid_utf8":
            _write_raw(writer, b'{"id":"0","ok":true,"value":"\xff"}\n')
        elif mode == "byte_order_mark":
            _write_raw(writer, b"\xef\xbb\xbf" + valid_bytes)
        elif mode == "oversized_line":
            _write_raw(writer, b" " * MAX_LINE_BYTES + b"\n")
        elif mode == "malformed_json":
            _write_raw(writer, b'{"id":}\n')
        elif mode == "early_eof":
            _write_raw(writer, b'{"id":"0","ok":true}')
        elif mode == "extra_response":
            _write_raw(writer, valid_bytes + valid_bytes)
        elif mode == "mismatched_request_id":
            valid_response["id"] = "unexpected"
            _write_raw(writer, _json_bytes(valid_response))
        else:
            raise RequestError(f"unknown protocol failure fixture {mode!r}")
        return 0
    finally:
        reader.close()
        writer.close()


def _accept_hello(reader: BinaryIO, writer: BinaryIO, role: str) -> None:
    request = _read_request(reader)
    _require_shape(request, "hello", {"protocols", "role", "worker_id"})
    if request["role"] != role:
        raise RequestError(f"expected role {role!r}")
    _send(writer, _hello_response(request))


def _hello_response(request: dict[str, object]) -> dict[str, object]:
    protocols = request.get("protocols")
    if not isinstance(protocols, list) or 1 not in protocols:
        raise RequestError("fixture requires protocol version 1")
    worker_id = request.get("worker_id")
    if not isinstance(worker_id, str):
        raise RequestError("hello requires a worker identity")
    capabilities = ["one_shot"] if request.get("role") == "implementation" else []
    return {
        "capabilities": capabilities,
        "id": request["id"],
        "ok": True,
        "protocol": 1,
        "sdk": None,
        "worker_id": worker_id,
    }


def _materialize(request: dict[str, object]) -> dict[str, object]:
    _require_shape(
        request,
        "materialize",
        {"source_dir", "dataset_parameters", "dataset_seed", "output_dir"},
    )
    parameters = _object(request["dataset_parameters"], "dataset_parameters")
    values = parameters.get("values")
    if not isinstance(values, list) or not values:
        raise RequestError("dataset_parameters.values must be a nonempty array")
    if any(type(value) is not int for value in values):
        raise RequestError("dataset_parameters.values must contain only integers")
    dataset_seed = request["dataset_seed"]
    if type(dataset_seed) is not int:
        raise RequestError("dataset_seed must be an integer")

    output_dir = Path(_string(request["output_dir"], "output_dir"))
    output_dir.mkdir(parents=True, exist_ok=True)
    payload = _json_bytes({"dataset_seed": dataset_seed, "values": values})
    _write_file(output_dir / "data.json", payload)
    manifest = {
        "complete": True,
        "files": [
            {
                "media_type": "application/json",
                "path": "data.json",
                "sha256": hashlib.sha256(payload).hexdigest(),
                "size_bytes": len(payload),
            }
        ],
        "schema_version": 1,
    }
    _write_file(output_dir / "dataset-manifest.json", _json_bytes(manifest))
    return {
        "dataset": {"manifest": "dataset-manifest.json"},
        "id": request["id"],
        "ok": True,
    }


def _prepare(request: dict[str, object]) -> dict[str, object]:
    _require_shape(
        request,
        "prepare",
        {
            "dataset_dir",
            "problem_contract_id",
            "problem_parameters",
            "implementation_parameters",
            "implementation_seed",
        },
    )
    dataset_dir = Path(_string(request["dataset_dir"], "dataset_dir"))
    dataset = _read_json_file(dataset_dir / "data.json")
    values = _object(dataset, "dataset").get("values")
    if not isinstance(values, list) or any(type(value) is not int for value in values):
        raise RequestError("dataset values must be an integer array")
    problem_parameters = _object(request["problem_parameters"], "problem_parameters")
    implementation_parameters = _object(
        request["implementation_parameters"], "implementation_parameters"
    )
    if implementation_parameters.get("algorithm") != "sum":
        raise RequestError("fixture implementation requires algorithm 'sum'")
    return {
        "problem_parameters": problem_parameters,
        "values": values,
    }


def _execute(
    request: dict[str, object], state: dict[str, object] | None
) -> dict[str, object]:
    _require_shape(request, "execute", {"result_dir"})
    if state is None:
        raise RequestError("prepare must succeed before execute")
    problem_parameters = _object(state["problem_parameters"], "problem_parameters")
    offset = problem_parameters.get("offset", 0)
    if type(offset) is not int:
        raise RequestError("problem_parameters.offset must be an integer")
    values = state["values"]
    if not isinstance(values, list):
        raise RequestError("prepared fixture state is invalid")
    answer = sum(values) + offset

    result_dir = Path(_string(request["result_dir"], "result_dir"))
    result_dir.mkdir(parents=True, exist_ok=True)
    manifest = {
        "data": {"answer": answer},
        "files": [],
        "schema": "schemas/result.json",
        "schema_version": 1,
    }
    _write_file(result_dir / "result.json", _json_bytes(manifest))
    return {
        "id": request["id"],
        "implementation_time_ns": None,
        "ok": True,
        "result": {"manifest": "result.json"},
        "statistics": {"input_count": len(values)},
    }


def _evaluate(request: dict[str, object]) -> dict[str, object]:
    _require_shape(
        request,
        "evaluate",
        {
            "dataset_dir",
            "problem_contract_id",
            "problem_parameters",
            "result_dir",
            "metrics_path",
        },
    )
    dataset = _object(
        _read_json_file(Path(_string(request["dataset_dir"], "dataset_dir")) / "data.json"),
        "dataset",
    )
    result = _object(
        _read_json_file(Path(_string(request["result_dir"], "result_dir")) / "result.json"),
        "result",
    )
    values = dataset.get("values")
    if not isinstance(values, list) or any(type(value) is not int for value in values):
        raise RequestError("dataset values must be an integer array")
    problem_parameters = _object(request["problem_parameters"], "problem_parameters")
    offset = problem_parameters.get("offset", 0)
    if type(offset) is not int:
        raise RequestError("problem_parameters.offset must be an integer")
    data = _object(result.get("data"), "result.data")
    answer = data.get("answer")
    if type(answer) is not int:
        raise RequestError("result.data.answer must be an integer")

    metrics = {
        "data": {"absolute_error": abs(answer - (sum(values) + offset))},
        "schema": "schemas/metrics.json",
        "schema_version": 1,
    }
    metrics_path = Path(_string(request["metrics_path"], "metrics_path"))
    metrics_path.parent.mkdir(parents=True, exist_ok=True)
    _write_file(metrics_path, _json_bytes(metrics))
    return {"id": request["id"], "ok": True}


def _require_shape(
    request: dict[str, object], method: str, fields: set[str]
) -> None:
    expected = fields | {"id", "method"}
    if request.get("method") != method or set(request) != expected:
        raise RequestError(f"invalid {method} request shape")


def _protocol_streams() -> tuple[BinaryIO, BinaryIO]:
    try:
        read_fd = int(os.environ[PROTOCOL_READ_FD_ENV])
        write_fd = int(os.environ[PROTOCOL_WRITE_FD_ENV])
    except (KeyError, ValueError) as error:
        raise RequestError("protocol descriptor environment is invalid") from error
    return (
        os.fdopen(read_fd, "rb", buffering=0),
        os.fdopen(write_fd, "wb", buffering=0),
    )


def _read_request(reader: BinaryIO) -> dict[str, object]:
    line = reader.readline(MAX_LINE_BYTES + 1)
    if not line:
        raise RequestError("protocol request stream ended")
    if len(line) > MAX_LINE_BYTES or not line.endswith(b"\n"):
        raise RequestError("protocol request is not one bounded line")
    value = json.loads(line)
    return _object(value, "request")


def _read_json_file(path: Path) -> object:
    with path.open("rb") as source:
        return json.load(source)


def _object(value: object, name: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise RequestError(f"{name} must be an object")
    return value


def _string(value: object, name: str) -> str:
    if not isinstance(value, str) or not value:
        raise RequestError(f"{name} must be a nonempty string")
    return value


def _json_bytes(value: object) -> bytes:
    encoded = json.dumps(
        value,
        allow_nan=False,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")
    return encoded + b"\n"


def _send(writer: BinaryIO, value: object) -> None:
    _write_raw(writer, _json_bytes(value))


def _write_raw(writer: BinaryIO, value: bytes) -> None:
    writer.write(value)
    writer.flush()


def _write_fragmented(writer: BinaryIO, value: bytes) -> None:
    for byte in value:
        writer.write(bytes([byte]))
        writer.flush()
        time.sleep(0.0001)


def _write_file(path: Path, value: bytes) -> None:
    with path.open("wb") as destination:
        destination.write(value)


def die(message: str) -> NoReturn:
    """Report a fixture invocation error without contaminating protocol output."""

    print(message, file=sys.stderr)
    raise SystemExit(2)
