"""A raw one-shot implementation and independent evaluator, without a package."""

from metewand_example_arithmetic import total

import hashlib
import json
import os
from pathlib import Path
import struct
import sys


def read_json(path):
    return json.loads(Path(path).read_bytes())


def write_json(path, value):
    Path(path).write_text(json.dumps(value, separators=(",", ":"), sort_keys=True))


def protocol_stream(direction, mode):
    if os.name == "nt":
        import msvcrt

        handle = int(os.environ[f"METEWAND_PROTOCOL_{direction}_HANDLE"])
        os.set_handle_inheritable(handle, False)
        flags = os.O_BINARY | os.O_NOINHERIT
        flags |= os.O_RDONLY if mode == "rb" else os.O_WRONLY
        descriptor = msvcrt.open_osfhandle(handle, flags)
    else:
        descriptor = int(os.environ[f"METEWAND_PROTOCOL_{direction}_FD"])
        os.set_inheritable(descriptor, False)
    return os.fdopen(descriptor, mode)


def main():
    role = sys.argv[1]
    representation = sys.argv[2] if len(sys.argv) > 2 else "list"
    assert representation in {"list", "packed"}
    reader = protocol_stream("READ", "rb")
    writer = protocol_stream("WRITE", "wb")
    print(f"{role} stdout", flush=True)
    print(f"{role} stderr", file=sys.stderr, flush=True)
    prepared = None
    offset = None
    for line in reader:
        request = json.loads(line)
        method = request["method"]
        response = {"id": request["id"], "ok": True}
        if method == "hello":
            assert request["role"] == role
            response.update(
                protocol=1,
                worker_id=request["worker_id"],
                capabilities=["one_shot"] if role == "implementation" else [],
                sdk=None,
            )
        elif method == "materialize":
            assert role == "dataset_materializer"
            source = Path(request["source_dir"]) / "source"
            values = read_json(source)["values"] if source.exists() else request["dataset_parameters"]["values"]
            assert all(type(value) is int and -(1 << 63) <= value < (1 << 63) for value in values)
            output = Path(request["output_dir"])
            write_json(output / "data.json", {"values": values, "dataset_seed": request["dataset_seed"]})
            payload = (output / "data.json").read_bytes()
            write_json(output / "dataset-manifest.json", {
                "schema_version": 1, "complete": True,
                "files": [{"path": "data.json", "media_type": "application/json", "sha256": hashlib.sha256(payload).hexdigest(), "size_bytes": len(payload)}],
            })
            response["dataset"] = {"manifest": "dataset-manifest.json"}
        elif method == "prepare":
            assert role == "implementation"
            values = read_json(Path(request["dataset_dir"]) / "data.json")["values"]
            prepared = struct.pack(f"<{len(values)}q", *values) if representation == "packed" else values
            offset = request["problem_parameters"]["offset"]
        elif method == "execute":
            assert role == "implementation" and prepared is not None
            values = (item[0] for item in struct.iter_unpack("<q", prepared)) if representation == "packed" else prepared
            write_json(
                Path(request["result_dir"]) / "result.json",
                {
                    "schema_version": 1,
                    "schema": "schemas/result.json",
                    "data": {"answer": total(values) + offset},
                    "files": [],
                },
            )
            response.update(
                result={"manifest": "result.json"},
                implementation_time_ns=None,
                statistics={},
            )
        elif method == "evaluate":
            assert role == "problem_evaluator"
            values = read_json(Path(request["dataset_dir"]) / "data.json")["values"]
            expected = sum(values) + request["problem_parameters"]["offset"]
            answer = read_json(Path(request["result_dir"]) / "result.json")["data"]["answer"]
            error = abs(answer - expected)
            write_json(
                request["metrics_path"],
                {
                    "schema_version": 1,
                    "schema": "schemas/metrics.json",
                    "data": {"absolute_error": error},
                },
            )
            response["verdict"] = "reached" if error == 0 else "invalid"
        elif method != "shutdown":
            raise ValueError(f"unsupported method: {method}")
        writer.write(json.dumps(response, separators=(",", ":")).encode() + b"\n")
        writer.flush()
        if method == "shutdown":
            return


if __name__ == "__main__":
    main()
