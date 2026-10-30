"""Shared canonical, identity, seed, and wire conformance vectors."""

import hashlib
import io
import unittest
from pathlib import Path

from binding_support import serve
from metewand import WorkerError
from metewand_adapter._artifacts import read_json
from metewand_adapter._canonical import canonical_bytes, parse_json
from metewand_adapter._worker import (
    ProtocolError,
    WorkerError as WireWorkerError,
    _encode_response,
)

ROOT = Path(__file__).resolve().parents[5]


def fixture(path):
    value = read_json(ROOT / "fixtures" / path)
    assert value["version"] == 1
    return value


class ConformanceTests(unittest.TestCase):
    def assert_identity(self, kind, source, identity):
        canonical = canonical_bytes(parse_json(source))
        self.assertEqual(canonical, source.encode())
        digest = hashlib.sha256(kind.encode() + b"\0" + canonical)
        self.assertEqual(f"mw1-{kind}-{digest.hexdigest()}", identity)

    def test_identity_vectors(self):
        for name, vector in fixture("identities/v1.json")["vectors"].items():
            with self.subTest(name):
                self.assert_identity(
                    vector["kind"], vector["canonical"], vector["identity"]
                )
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
            self.assert_identity(
                "execution-policy", vector["canonical"], vector["identity"]
            )
        source = (
            (ROOT / "fixtures/identities/dataset-derivation-v1.json")
            .read_text()
            .rstrip("\n")
        )
        self.assert_identity(
            "dataset-derivation",
            source,
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
                        vectors["domain"].encode()
                        + b"\0"
                        + role.encode()
                        + b"\0"
                        + canonical
                    ).digest()
                    self.assertEqual(digest.hex(), vector["digest"])
                    self.assertEqual(
                        int.from_bytes(digest[:8], "big") >> 11, vector["value"]
                    )

    def test_protocol_response_vectors(self):
        vectors = fixture("worker-protocol/v1/golden.json")
        for vector in vectors["responses"]:
            self.assertEqual(
                _encode_response(parse_json(vector["frame"])), vector["frame"].encode()
            )

    def test_shared_callback_errors(self):
        vectors = fixture("worker-protocol/v1/errors.json")
        greeting = fixture("worker-protocol/v1/golden.json")["sessions"][1][
            "exchanges"
        ][0]
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
                    greeting["request"].encode()
                    + canonical_bytes(request)
                    + b"\n"
                    + b'{"id":"end","method":"shutdown"}\n'
                )
                output = io.BytesIO()
                serve(
                    {"materialize": fail},
                    "dataset_materializer",
                    io.BytesIO(calls),
                    output,
                )
                self.assertEqual(
                    output.getvalue().splitlines(keepends=True)[1],
                    vector["frame"].encode(),
                )
        for vector in vectors["rejected"]:
            with self.subTest(vector), self.assertRaises((ValueError, TypeError)):
                WireWorkerError(**vector)

    def test_shared_rejected_frames(self):
        for vector in fixture("worker-protocol/v1/golden.json")["rejected_frames"]:
            with self.subTest(vector["name"]), self.assertRaises(ProtocolError):
                serve(
                    {
                        "prepare": lambda context: None,
                        "execute": lambda state, context: {},
                    },
                    "implementation",
                    io.BytesIO(bytes.fromhex(vector["hex"])),
                    io.BytesIO(),
                )
