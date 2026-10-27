"""Shared compatibility vectors and test-only seed transcripts."""

import hashlib
import io
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from metewand import (
    __version__,
    Implementation,
    WorkerError,
    canonical_bytes,
    file_entry,
    parse_json,
    read_json,
    write_dataset_manifest,
    write_json,
    write_metrics,
    write_result,
)
from metewand._worker import ProtocolError, _encode_response, _serve

ROOT = Path(__file__).resolve().parents[3]


def fixture(path):
    value = read_json(ROOT / "fixtures" / path)
    assert value["version"] == 1
    return value


class GoldenImplementation(Implementation):
    def prepare(self, dataset, contract, problem, implementation, seed):
        self.seed = seed

    def execute(self, budget, observation_control):
        assert budget is None
        assert observation_control == ({"iterations": 4} if getattr(self, "fresh_sequence", False) else None)
        return {"seed": self.seed}

    def write_result(self, directory):
        write_json(directory / "answer.json", {"answer": 6})
        return write_result(
            directory,
            {"answer": 6},
            schema="schemas/result.json",
            files=[file_entry(directory, "answer.json", "application/json")],
        )

    def reset(self):
        self.__dict__.clear()


def materialize(source, parameters, seed, output):
    return write_dataset_manifest(output, [])


def evaluate(dataset, contract, parameters, result, metrics):
    write_metrics(metrics, {"error": 0}, schema="schemas/metrics.json")
    return parameters["verdict"]


class ConformanceTests(unittest.TestCase):
    def assert_identity(self, kind, source, identity):
        canonical = canonical_bytes(parse_json(source))
        self.assertEqual(canonical, source.encode())
        digest = hashlib.sha256(kind.encode() + b"\0" + canonical)
        self.assertEqual(f"mw1-{kind}-{digest.hexdigest()}", identity)

    def test_identity_vectors(self):
        for name, vector in fixture("identities/v1.json")["vectors"].items():
            with self.subTest(name):
                self.assert_identity(vector["kind"], vector["canonical"], vector["identity"])
        for vector in fixture("identities/diagnostics-v1.json")["vectors"]:
            diagnostic = {"details": None, **vector["diagnostic"]}
            self.assertEqual(
                canonical_bytes({"value": diagnostic, "version": 1}),
                vector["canonical"].encode(),
            )
            self.assert_identity("diagnostic", vector["canonical"], vector["id"])
        policies = read_json(ROOT / "fixtures/identities/concurrency-v1.json")
        self.assertEqual(policies["identity_version"], 1)
        for vector in policies["policies"]:
            self.assert_identity("execution-policy", vector["canonical"], vector["identity"])
        source = (ROOT / "fixtures/identities/dataset-derivation-v1.json").read_text().rstrip("\n")
        self.assert_identity(
            "dataset-derivation", source,
            "mw1-dataset-derivation-875a677f1d0ce2b6b77bc5a03f213774188e0ab45f4b8fc18cbf754de70847c4",
        )

    def test_seed_vectors(self):
        vectors = fixture("seeds/v1.json")
        fields = {
            "dataset": ("dataset_definition", "dataset_parameters", "experiment_seed"),
            "implementation": (
                "dataset_configuration",
                "experiment_seed",
                "implementation_repetition",
                "problem_configuration",
            ),
            "scheduling": (
                "benchmark_name",
                "experiment_name",
                "experiment_seed",
                "scheduling_policy_version",
            ),
        }
        for role, keys in fields.items():
            for vector in vectors[role]:
                with self.subTest(vector["name"]):
                    canonical = canonical_bytes({key: vector[key] for key in keys})
                    self.assertEqual(canonical, vector["canonical_fields"].encode())
                    digest = hashlib.sha256(
                        vectors["domain"].encode() + b"\0" + role.encode() + b"\0" + canonical
                    ).digest()
                    self.assertEqual(digest.hex(), vector["digest"])
                    self.assertEqual(int.from_bytes(digest[:8], "big") >> 11, vector["value"])

    def test_protocol_sessions_and_publication(self):
        vectors = fixture("worker-protocol/v1/golden.json")
        for session in vectors["sessions"]:
            with self.subTest(session["role"]), tempfile.TemporaryDirectory() as directory:
                role = session["role"]
                worker = {
                    "implementation": GoldenImplementation(),
                    "dataset_materializer": materialize,
                    "problem_evaluator": evaluate,
                }[role]
                if role == "implementation":
                    worker.fresh_sequence = session.get("fresh_sequence", False)
                calls = [
                    step["request"].replace("@ROOT@", directory).encode()
                    for step in session["exchanges"]
                ]
                expected = []
                for step in session["exchanges"]:
                    frame = step["response"].replace("@SDK@", "metewand-python").encode()
                    response = parse_json(frame)
                    if response.get("sdk"):
                        # Package releases do not change the protocol fixture version.
                        sdk = response["sdk"]
                        frame = frame.replace(
                            canonical_bytes(sdk),
                            canonical_bytes({**sdk, "version": __version__}),
                            1,
                        )
                    expected.append(frame)
                expected = b"".join(expected)

                class InspectingWriter(io.BytesIO):
                    def write(self, frame):
                        response = parse_json(frame)
                        if response["ok"]:
                            if "result" in response:
                                self.assert_result(response)
                            if "dataset" in response:
                                read_json(Path(directory) / response["dataset"]["manifest"])
                            if "verdict" in response:
                                read_json(Path(directory) / "metrics.json")
                        return super().write(frame)

                    def assert_result(self, response):
                        manifest = read_json(Path(directory) / response["result"]["manifest"])
                        assert manifest["data"] == {"answer": 6}
                        for entry in manifest["files"]:
                            assert read_json(Path(directory) / entry["path"]) == {"answer": 6}
                            assert file_entry(directory, entry["path"], entry["media_type"]) == entry

                output = InspectingWriter()
                with patch("metewand._worker.time.perf_counter_ns", side_effect=[100, 125]):
                    _serve(worker, role, io.BytesIO(b"".join(calls)), output, fresh_sequence=session.get("fresh_sequence", False))
                self.assertEqual(output.getvalue(), expected)
        for vector in vectors["responses"]:
            self.assertEqual(
                _encode_response(parse_json(vector["frame"])), vector["frame"].encode()
            )

    def test_shared_callback_errors(self):
        vectors = fixture("worker-protocol/v1/errors.json")
        greeting = fixture("worker-protocol/v1/golden.json")["sessions"][1]["exchanges"][0]
        for vector in vectors["accepted"]:
            with self.subTest(vector["error"]["code"]):

                def fail(*args):
                    raise WorkerError(**vector["error"])

                request = {
                    "id": "failure",
                    "method": "materialize",
                    "source_dir": "/source",
                    "dataset_parameters": {},
                    "dataset_seed": 0,
                    "output_dir": "/output",
                }
                calls = (
                    greeting["request"].encode() + canonical_bytes(request) + b"\n"
                    + b'{"id":"end","method":"shutdown"}\n'
                )
                output = io.BytesIO()
                _serve(fail, "dataset_materializer", io.BytesIO(calls), output)
                self.assertEqual(
                    output.getvalue().splitlines(keepends=True)[1],
                    vector["frame"].encode(),
                )
        for vector in vectors["rejected"]:
            with self.subTest(vector), self.assertRaises((ValueError, TypeError)):
                WorkerError(**vector)

    def test_shared_rejected_frames(self):
        for vector in fixture("worker-protocol/v1/golden.json")["rejected_frames"]:
            with self.subTest(vector["name"]), self.assertRaises(ProtocolError):
                _serve(
                    GoldenImplementation(), "implementation",
                    io.BytesIO(bytes.fromhex(vector["hex"])), io.BytesIO(),
                )
