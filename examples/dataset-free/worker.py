"""Sum integers from one through n without an external dataset."""

import json
import os
from pathlib import Path
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
    reader = protocol_stream("READ", "rb")
    writer = protocol_stream("WRITE", "wb")
    print(f"{role} stdout", flush=True)
    print(f"{role} stderr", file=sys.stderr, flush=True)
    prepared = None
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
        elif method == "prepare":
            assert role == "implementation"
            assert list(Path(request["dataset_dir"]).iterdir()) == []
            prepared = request["problem_parameters"]["n"]
        elif method == "execute":
            assert role == "implementation" and prepared is not None
            write_json(
                Path(request["result_dir"]) / "result.json",
                {
                    "schema_version": 1,
                    "schema": "schemas/result.json",
                    "data": {"answer": sum(range(1, prepared + 1))},
                    "files": [],
                },
            )
            response.update(
                result={"manifest": "result.json"},
                implementation_time_ns=None,
                statistics={},
            )
        elif method == "reset":
            assert role == "implementation"
            prepared = None
        elif method == "evaluate":
            assert role == "problem_evaluator"
            assert list(Path(request["dataset_dir"]).iterdir()) == []
            n = request["problem_parameters"]["n"]
            expected = n * (n + 1) // 2
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
