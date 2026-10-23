"""Gate 2 process transport for the three scientific smoke fixtures."""

import json
import os
import sys
from pathlib import Path


def read_json(path):
    return json.loads(Path(path).read_bytes())


def write_json(path, value):
    Path(path).write_text(json.dumps(value, separators=(",", ":"), sort_keys=True))


def main():
    role = sys.argv[1]
    reader = os.fdopen(int(os.environ["METEWAND_PROTOCOL_READ_FD"]), "rb")
    writer = os.fdopen(int(os.environ["METEWAND_PROTOCOL_WRITE_FD"]), "wb")
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
            prepared = read_json(next(Path(request["dataset_dir"]).rglob("data.json")))
        elif method == "execute":
            assert role == "implementation" and prepared is not None
            from solver import solve

            result = solve(prepared)
            write_json(
                Path(request["result_dir"]) / "result.json",
                {
                    "schema_version": 1,
                    "schema": "schemas/result.json",
                    "data": result,
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
            from evaluator import evaluate

            data = read_json(next(Path(request["dataset_dir"]).rglob("data.json")))
            result = read_json(Path(request["result_dir"]) / "result.json")["data"]
            metrics, verdict = evaluate(data, result)
            write_json(
                request["metrics_path"],
                {
                    "schema_version": 1,
                    "schema": "schemas/metrics.json",
                    "data": metrics,
                },
            )
            response["verdict"] = verdict
        elif method != "shutdown":
            raise ValueError(f"unsupported method: {method}")
        writer.write(json.dumps(response, separators=(",", ":")).encode() + b"\n")
        writer.flush()
        if method == "shutdown":
            return


if __name__ == "__main__":
    main()
