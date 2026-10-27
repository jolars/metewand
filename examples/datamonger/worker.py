"""A raw one-shot implementation and independent evaluator, without a package."""

import csv
import hashlib
import json
import os
import struct
import sys
from decimal import Decimal
from pathlib import Path


def read_json(path):
    return json.loads(Path(path).read_bytes())


def write_json(path, value):
    Path(path).write_text(json.dumps(value, separators=(",", ":"), sort_keys=True))


def main():
    role = sys.argv[1]
    representation = sys.argv[2] if len(sys.argv) > 2 else "list"
    assert representation in {"list", "packed"}
    reader = os.fdopen(int(os.environ["METEWAND_PROTOCOL_READ_FD"]), "rb")
    writer = os.fdopen(int(os.environ["METEWAND_PROTOCOL_WRITE_FD"]), "wb")
    print(f"{role} stdout", flush=True)
    print(f"{role} stderr", file=sys.stderr, flush=True)
    prepared = None
    offset = None
    input_digest = None
    dataset_directory = None
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
            source = Path(request["source_dir"]) / "artifacts/data"
            parameters = request["dataset_parameters"]
            with source.open(newline="", encoding="utf-8") as stream:
                rows = csv.reader(stream)
                next(rows)
                scaled = [
                    Decimal(row[parameters["column"]]) * parameters["scale"]
                    for row in rows
                ]
            assert all(
                value.is_finite() and value == value.to_integral_value()
                for value in scaled
            )
            values = [int(value) for value in scaled]
            assert all(
                type(value) is int and -(1 << 63) <= value < (1 << 63)
                for value in values
            )
            output = Path(request["output_dir"])
            write_json(
                output / "data.json",
                {"values": values, "dataset_seed": request["dataset_seed"]},
            )
            payload = (output / "data.json").read_bytes()
            write_json(
                output / "dataset-manifest.json",
                {
                    "schema_version": 1,
                    "complete": True,
                    "files": [
                        {
                            "path": "data.json",
                            "media_type": "application/json",
                            "sha256": hashlib.sha256(payload).hexdigest(),
                            "size_bytes": len(payload),
                        }
                    ],
                },
            )
            response["dataset"] = {"manifest": "dataset-manifest.json"}
        elif method == "prepare":
            assert role == "implementation"
            dataset_directory = request["dataset_dir"]
            input_digest = hashlib.sha256(
                (Path(dataset_directory) / "data.json").read_bytes()
            ).hexdigest()
            values = read_json(Path(dataset_directory) / "data.json")["values"]
            prepared = (
                struct.pack(f"<{len(values)}q", *values)
                if representation == "packed"
                else values
            )
            offset = request["problem_parameters"]["offset"]
        elif method == "execute":
            assert role == "implementation" and prepared is not None
            values = (
                (item[0] for item in struct.iter_unpack("<q", prepared))
                if representation == "packed"
                else prepared
            )
            write_json(
                Path(request["result_dir"]) / "result.json",
                {
                    "schema_version": 1,
                    "schema": "schemas/result.json",
                    "data": {"answer": sum(values) + offset},
                    "files": [],
                },
            )
            response.update(
                result={"manifest": "result.json"},
                implementation_time_ns=None,
                statistics={
                    "input_sha256": input_digest,
                    "dataset_dir": dataset_directory,
                },
            )
        elif method == "evaluate":
            assert role == "problem_evaluator"
            values = read_json(Path(request["dataset_dir"]) / "data.json")["values"]
            expected = sum(values) + request["problem_parameters"]["offset"]
            answer = read_json(Path(request["result_dir"]) / "result.json")["data"][
                "answer"
            ]
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
