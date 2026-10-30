import io
import json
import os
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from unittest.mock import patch

from binding_support import METADATA, WORKER_ID, exchange, hello, request, serve
from metewand import Artifact, Dataset, Evaluation, Result, WorkerError
from metewand_adapter import serve_bindings
from metewand_adapter._artifacts import (
    file_entry,
    read_json,
    write_dataset_manifest,
    write_json,
    write_metrics,
    write_result,
)
from metewand_adapter._canonical import CanonicalJsonError, canonical_bytes, parse_json
from metewand_adapter._worker import MAX_LINE_BYTES, ProtocolError, _encode_response

ROOT = Path(__file__).resolve().parents[5]


def prepare(directory="dataset"):
    return request(
        "prepare",
        dataset_dir=str(directory),
        problem_contract_id="contract",
        problem_parameters={"offset": 2},
        implementation_parameters={"iterations": 4},
        implementation_seed=23,
    )


def callbacks(**overrides):
    return dict(
        {"prepare": lambda context: None, "execute": lambda state, context: {}},
        **overrides,
    )


class CanonicalTests(unittest.TestCase):
    def test_shared_vectors(self):
        vectors = json.loads((ROOT / "fixtures/canonical-json/v1.json").read_text())
        for vector in vectors["accepted"]:
            with self.subTest(vector["name"]):
                self.assertEqual(
                    canonical_bytes(parse_json(vector["input"])),
                    vector["canonical"].encode(),
                )
        for vector in vectors["rejected"]:
            with self.subTest(vector["name"]), self.assertRaises(CanonicalJsonError):
                parse_json(vector["input"])

    def test_python_values_are_not_coerced(self):
        for value in [
            float("nan"),
            float("inf"),
            2**53,
            float(2**53),
            {1: "key"},
            (1, 2),
            {1, 2},
            "\ud800",
            {"\udfff": 1},
        ]:
            with self.subTest(value=repr(value)), self.assertRaises(CanonicalJsonError):
                canonical_bytes(value)
        for value in [b'"\xff"', b"\xef\xbb\xbf{}"]:
            with self.assertRaises(CanonicalJsonError):
                parse_json(value)
        cycle = []
        cycle.append(cycle)
        with self.assertRaises(CanonicalJsonError):
            canonical_bytes(cycle)


class ArtifactTests(unittest.TestCase):
    def test_complete_envelopes_and_file_inventory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_json(root / "data.json", {"values": [1, 2, 3]})
            entry = file_entry(
                root,
                "data.json",
                "application/json",
                representation={"shape": [3], "data_type": "json"},
            )
            self.assertEqual(entry["size_bytes"], 18)
            self.assertEqual(
                entry["sha256"],
                "abe0a9abcc957f9f5aa04d68c4f49fadee7c4b62343e403a3a1c0b646519d53c",
            )
            manifest = write_dataset_manifest(root, [entry])
            self.assertEqual(
                read_json(root / manifest),
                {"schema_version": 1, "complete": True, "files": [entry]},
            )
            result = write_result(root, {"answer": 6}, schema="schemas/result.json")
            self.assertEqual(
                read_json(root / result),
                {
                    "schema_version": 1,
                    "schema": "schemas/result.json",
                    "data": {"answer": 6},
                    "files": [],
                },
            )
            write_metrics(
                root / "metrics.json", {"error": 0}, schema="schemas/metrics.json"
            )
            self.assertEqual(read_json(root / "metrics.json")["data"], {"error": 0})

    def test_envelopes_can_use_the_selected_contract_schema(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result = write_result(root, {"answer": 6})
            self.assertEqual(
                read_json(root / result),
                {"schema_version": 1, "data": {"answer": 6}, "files": []},
            )
            write_metrics(root / "metrics.json", {"error": 0})
            self.assertEqual(
                read_json(root / "metrics.json"),
                {"schema_version": 1, "data": {"error": 0}},
            )

    def test_paths_and_metadata_are_explicit(self):
        with (
            tempfile.TemporaryDirectory() as directory,
            tempfile.TemporaryDirectory() as outside,
        ):
            root = Path(directory)
            (root / "data").write_bytes(b"x")
            for path in ["../escape", "/absolute", "a/./b", "a//b", "a\\b", ""]:
                with self.subTest(path=path), self.assertRaises(ValueError):
                    file_entry(root, path, "application/octet-stream")
            try:
                (root / "link").symlink_to(Path(outside), target_is_directory=True)
            except OSError:
                if os.name != "nt":
                    raise
                subprocess.run(
                    ["cmd", "/d", "/c", "mklink", "/J", str(root / "link"), outside],
                    check=True,
                    capture_output=True,
                )
            with self.assertRaises(ValueError):
                write_result(root, {}, schema="s.json", manifest="link/escape.json")
            with self.assertRaises(ValueError):
                file_entry(root, "data", "text/plain", representation={"shape": [-1]})
            entry = file_entry(root, "data", "text/plain")
            with self.assertRaises(ValueError):
                write_dataset_manifest(root, [entry, entry])
            with self.assertRaises(ValueError):
                write_result(root, [], schema="s.json")


class WorkerTests(unittest.TestCase):
    def test_lifecycle_contexts_state_cleanup_and_solve_timing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            state = {"answer": 6}
            events = []
            contexts = []

            def prepare_callback(context):
                events.append("prepare")
                contexts.append(context)
                self.assertEqual(context.dataset_dir, root)
                self.assertEqual(context.parameters, {"iterations": 4})
                self.assertEqual(context.problem_parameters, {"offset": 2})
                self.assertEqual(context.seed, 23)
                return state

            def execute(prepared, context):
                events.append("execute")
                self.assertIs(prepared, state)
                self.assertIs(context, contexts[0])
                self.assertEqual(context.output_dir, root)
                self.assertIsNone(context.budget)
                self.assertIsNone(context.observation_control)
                return state["answer"] + context.problem_parameters["offset"]

            def statistics(output, context):
                events.append("statistics")
                return {"iterations": context.parameters["iterations"]}

            def result(output, context):
                events.append("result")
                return Result({"answer": output})

            def reset(prepared, context):
                events.append("reset")
                self.assertIs(prepared, state)

            def shutdown(prepared, context):
                events.append("shutdown")
                self.assertIsNone(prepared)

            ticks = iter([100, 125])

            def clock():
                events.append("clock")
                return next(ticks)

            with patch(
                "metewand_adapter._worker.time.perf_counter_ns", side_effect=clock
            ):
                replies = exchange(
                    dict(
                        prepare=prepare_callback,
                        execute=execute,
                        statistics=statistics,
                        result=result,
                        reset=reset,
                        shutdown=shutdown,
                    ),
                    "implementation",
                    [
                        hello(),
                        prepare(root),
                        request("execute", result_dir=directory),
                        request("reset"),
                        request("execute", result_dir=directory),
                        request("shutdown"),
                    ],
                )
            self.assertEqual(
                events,
                [
                    "prepare",
                    "clock",
                    "execute",
                    "clock",
                    "statistics",
                    "result",
                    "reset",
                    "shutdown",
                ],
            )
            self.assertEqual(
                replies[0],
                {
                    "id": "0",
                    "ok": True,
                    "protocol": 1,
                    "worker_id": WORKER_ID,
                    "sdk": METADATA,
                    "capabilities": ["managed_outputs", "one_shot", "fresh_sequence"],
                },
            )
            self.assertEqual(replies[2]["implementation_time_ns"], 25)
            self.assertEqual(replies[2]["statistics"], {"iterations": 4})
            self.assertEqual(
                read_json(root / replies[2]["draft"]),
                {
                    "schema_version": 1,
                    "kind": "result",
                    "data": {"answer": 8},
                    "files": [],
                },
            )
            self.assertEqual(replies[4]["error"]["code"], "invalid_request")
            self.assertEqual(
                (root / replies[2]["draft"]).read_bytes(),
                b'{"data":{"answer":8},"files":[],"kind":"result","schema_version":1}',
            )

    def test_result_files_are_declared_without_package_computed_hashes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def execute(state, context):
                (context.output_dir / "coefficients.bin").write_bytes(b"coefficients")
                return Result(
                    {"answer": 8},
                    [
                        Artifact(
                            Path("coefficients.bin"),
                            "application/octet-stream",
                            {"shape": [3], "data_type": "float64"},
                        )
                    ],
                )

            replies = exchange(
                callbacks(execute=execute),
                "implementation",
                [
                    hello(),
                    prepare(root),
                    request("execute", result_dir=directory),
                    request("shutdown"),
                ],
            )
            draft = read_json(root / replies[2]["draft"])
            self.assertEqual(
                draft["files"],
                [
                    {
                        "path": "coefficients.bin",
                        "media_type": "application/octet-stream",
                        "representation": {"shape": [3], "data_type": "float64"},
                    }
                ],
            )
            self.assertFalse((root / "result.json").exists())

    def test_materializer_context_and_file_only_dataset(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def materialize(context):
                self.assertEqual(context.source_dir, root)
                self.assertEqual(context.output_dir, root)
                self.assertEqual(context.parameters, {"n": 3})
                self.assertEqual(context.seed, 17)
                (context.output_dir / "data.bin").write_bytes(b"data")
                return Dataset(files=[Artifact("data.bin", "application/octet-stream")])

            replies = exchange(
                {"materialize": materialize},
                "dataset_materializer",
                [
                    hello("dataset_materializer"),
                    request(
                        "materialize",
                        source_dir=directory,
                        dataset_parameters={"n": 3},
                        dataset_seed=17,
                        output_dir=directory,
                    ),
                    request("shutdown"),
                ],
            )
            draft = read_json(root / replies[1]["draft"])
            self.assertEqual(draft["kind"], "dataset")
            self.assertIsNone(draft["data"])
            self.assertEqual(
                draft["files"],
                [{"path": "data.bin", "media_type": "application/octet-stream"}],
            )

    def test_evaluator_context_and_scientific_verdicts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            rules = {
                "validity": {"finite": True},
                "one_shot_completion": {"error": 0.25},
            }

            def evaluate(context):
                self.assertEqual(context.semantics, rules)
                self.assertEqual(context.parameters, {"offset": 2})
                self.assertEqual(context.problem_parameters, {"offset": 2})
                self.assertEqual(context.dataset_dir, root)
                self.assertEqual(context.result_dir, root)
                self.assertEqual(context.metrics_path, root / "assigned.json")
                return Evaluation({"error": 0.2}, "reached")

            call = request(
                "evaluate",
                dataset_dir=directory,
                problem_contract_id="contract",
                problem_parameters={"offset": 2},
                problem_semantics=rules,
                result_dir=directory,
                metrics_path=str(root / "assigned.json"),
            )
            replies = exchange(
                {"evaluate": evaluate},
                "problem_evaluator",
                [hello("problem_evaluator"), call, request("shutdown")],
            )
            self.assertEqual(
                replies[0]["capabilities"], ["managed_outputs", "problem_semantics"]
            )
            self.assertEqual(replies[1]["verdict"], "reached")
            self.assertEqual(
                read_json(root / replies[1]["draft"])["data"], {"error": 0.2}
            )
            for invalid in [None, [], True]:
                replies = exchange(
                    {"evaluate": evaluate},
                    "problem_evaluator",
                    [
                        hello("problem_evaluator"),
                        dict(call, problem_semantics=invalid),
                        request("shutdown"),
                    ],
                )
                self.assertEqual(replies[1]["error"]["code"], "invalid_request")

    def test_evaluation_values_must_match_declared_rules(self):
        with tempfile.TemporaryDirectory() as directory:
            call = request(
                "evaluate",
                dataset_dir=directory,
                problem_contract_id="contract",
                problem_parameters={},
                problem_semantics={},
                result_dir=directory,
                metrics_path=str(Path(directory) / "metrics.json"),
            )
            for semantics, output, valid in [
                ({}, {"error": 0}, True),
                ({}, Evaluation({}, "not_required"), True),
                ({}, Evaluation({}, "invalid"), True),
                ({}, Evaluation({}, "reached"), False),
                ({"validity": {"finite": True}}, {}, False),
                (
                    {"one_shot_completion": {"error": 0}},
                    Evaluation({}, "not_required"),
                    False,
                ),
                (
                    {"one_shot_completion": {"error": 0}},
                    Evaluation({}, "not_reached"),
                    True,
                ),
                ({}, Evaluation({}, "invented"), False),
                ({}, Evaluation([], "invalid"), False),
                ({}, Evaluation({"x": float("nan")}, "invalid"), False),
            ]:
                with (
                    self.subTest(semantics=semantics, output=output),
                    redirect_stderr(io.StringIO()),
                ):
                    replies = exchange(
                        {"evaluate": lambda context: output},
                        "problem_evaluator",
                        [
                            hello("problem_evaluator"),
                            dict(call, problem_semantics=semantics),
                            request("shutdown"),
                        ],
                    )
                self.assertEqual(replies[1]["ok"], valid)

    def test_applicability_is_optional_and_validates_decisions(self):
        check = request(
            "check_applicability",
            dataset_dir="dataset",
            dataset_manifest="dataset/manifest.json",
            problem_contract_id="contract",
            problem_parameters={},
            implementation_parameters={},
        )
        for decision in [
            dict(outcome="unchecked", code="pending", reason="Pending"),
            dict(outcome="applicable", code="", reason="Supported"),
            dict(
                outcome="applicable", code="supported", reason="Supported", details=[]
            ),
        ]:
            with redirect_stderr(io.StringIO()):
                replies = exchange(
                    callbacks(check_applicability=lambda context: decision),
                    "implementation",
                    [hello(), check, request("shutdown")],
                )
            self.assertIn("applicability", replies[0]["capabilities"])
            self.assertFalse(replies[1]["ok"])
        replies = exchange(
            callbacks(), "implementation", [hello(), check, request("shutdown")]
        )
        self.assertNotIn("applicability", replies[0]["capabilities"])
        self.assertEqual(replies[1]["error"]["code"], "unsupported_operation")

    def test_failures_consume_preparation_but_preserve_state_for_shutdown(self):
        for failure in ["execute", "result"]:
            with tempfile.TemporaryDirectory() as directory:
                state = {"prepared": True}
                cleaned = []

                def execute(prepared, context):
                    if failure == "execute":
                        raise WorkerError("operation_failed", "Solve failed.")
                    return {"answer": 8}

                def convert(output, context):
                    return {"answer": float("nan")}

                with redirect_stderr(io.StringIO()):
                    replies = exchange(
                        callbacks(
                            prepare=lambda context: state,
                            execute=execute,
                            result=convert,
                            shutdown=lambda prepared, context: cleaned.append(prepared),
                        ),
                        "implementation",
                        [
                            hello(),
                            prepare(directory),
                            request("execute", result_dir=directory),
                            request("execute", result_dir=directory),
                            request("shutdown"),
                        ],
                    )
                self.assertEqual(replies[2]["error"]["code"], "operation_failed")
                self.assertEqual(replies[3]["error"]["code"], "invalid_request")
                self.assertEqual(cleaned, [state])

    def test_failed_prepare_and_reset_clear_prepared_state(self):
        for failing in ["prepare", "reset"]:
            prepared = []

            def prepare_callback(context):
                if prepared:
                    raise WorkerError("operation_failed", "Preparation failed.")
                prepared.append(True)

            def reset(state, context):
                raise WorkerError("operation_failed", "Reset failed.")

            replies = exchange(
                callbacks(prepare=prepare_callback, reset=reset),
                "implementation",
                [
                    hello(),
                    prepare(),
                    prepare() if failing == "prepare" else request("reset"),
                    request("execute", result_dir="result"),
                    request("shutdown"),
                ],
            )
            self.assertTrue(replies[1]["ok"])
            self.assertEqual(replies[2]["error"]["code"], "operation_failed")
            self.assertEqual(replies[3]["error"]["code"], "invalid_request")

    def test_worker_errors_and_unexpected_callback_failures(self):
        for error in [
            WorkerError("operation_failed", "bad data", {"index": 2}),
            RuntimeError("broken"),
        ]:

            def fail(context):
                raise error

            log = io.StringIO()
            with redirect_stderr(log):
                replies = exchange(
                    {"materialize": fail},
                    "dataset_materializer",
                    [
                        hello("dataset_materializer"),
                        request(
                            "materialize",
                            source_dir="source",
                            dataset_parameters={},
                            dataset_seed=0,
                            output_dir="output",
                        ),
                        request("shutdown"),
                    ],
                )
            self.assertEqual(replies[1]["error"]["code"], "operation_failed")
            if isinstance(error, WorkerError):
                self.assertEqual(
                    replies[1]["error"],
                    {
                        "code": "operation_failed",
                        "message": "bad data",
                        "details": {"index": 2},
                    },
                )
                self.assertEqual(log.getvalue(), "")
            else:
                self.assertIn("RuntimeError: broken", log.getvalue())
                self.assertIn("Traceback", replies[1]["error"]["message"])
            self.assertTrue(replies[2]["ok"])

    def test_unexpected_exception_text_is_safe_and_bounded(self):
        for message in ["bad \ud800 text", "x" * MAX_LINE_BYTES + " final detail"]:

            def fail(context):
                raise RuntimeError(message)

            with redirect_stderr(io.StringIO()):
                replies = exchange(
                    {"materialize": fail},
                    "dataset_materializer",
                    [
                        hello("dataset_materializer"),
                        request(
                            "materialize",
                            source_dir="source",
                            dataset_parameters={},
                            dataset_seed=0,
                            output_dir="output",
                        ),
                        request("shutdown"),
                    ],
                )
            error = replies[1]["error"]
            self.assertEqual(error["code"], "operation_failed")
            self.assertLess(len(canonical_bytes(error)), 65536)
            if len(message) > MAX_LINE_BYTES:
                self.assertIn("[traceback truncated]", error["message"])
                self.assertIn("final detail", error["message"])
            else:
                self.assertIn("bad \\ud800 text", error["message"])

    def test_invalid_requests_do_not_reach_callbacks(self):
        def unexpected(*args):
            self.fail("Malformed requests must not invoke callbacks.")

        for bad in [
            dict(prepare(), implementation_seed=True),
            request("execute", result_dir="result", budget={}),
            request("reset", extra=True),
            request("shutdown", extra=True),
            request("execute_stream"),
            hello(),
        ]:
            replies = exchange(
                callbacks(prepare=unexpected, execute=unexpected),
                "implementation",
                [hello(), bad, request("shutdown")],
            )
            self.assertFalse(replies[1]["ok"])
            self.assertIn(
                replies[1]["error"]["code"],
                ["invalid_request", "unsupported_operation"],
            )

    def test_handshake_rejection(self):
        for changes in [
            {"role": "problem_evaluator"},
            {"protocols": [2]},
            {"protocols": [1, 1]},
            {"worker_id": "wrong"},
            {"extra": 1},
        ]:
            replies = exchange(callbacks(), "implementation", [hello(**changes)])
            self.assertFalse(replies[0]["ok"])

    def test_malformed_frames_are_terminal(self):
        for data in [
            b'{"id":"1","id":"2"}\n',
            b"{}",
            b"\xff\n",
            b"\xef\xbb\xbf{}\n",
            b"{}\n",
            b"NaN\n",
            b"x" * MAX_LINE_BYTES + b"\n",
        ]:
            with self.subTest(data=data[:30]), self.assertRaises(ProtocolError):
                serve(callbacks(), "implementation", io.BytesIO(data), io.BytesIO())

    def test_frame_limit_includes_newline(self):
        value = {"text": ""}
        overhead = len(_encode_response(value))
        value["text"] = "a" * (MAX_LINE_BYTES - overhead)
        self.assertEqual(len(_encode_response(value)), MAX_LINE_BYTES)
        value["text"] += "a"
        with self.assertRaises(ValueError):
            _encode_response(value)
        greeting = canonical_bytes(hello())
        source = greeting + b" " * (MAX_LINE_BYTES - len(greeting) - 1) + b"\n"
        source += canonical_bytes(request("shutdown")) + b"\n"
        output = io.BytesIO()
        serve(callbacks(), "implementation", io.BytesIO(source), output)
        self.assertEqual(len(output.getvalue().splitlines()), 2)

    def test_oversized_callback_failure_is_bounded(self):
        def fail(context):
            raise WorkerError(
                "operation_failed", "large", {"text": "x" * MAX_LINE_BYTES}
            )

        replies = exchange(
            {"materialize": fail},
            "dataset_materializer",
            [
                hello("dataset_materializer"),
                request(
                    "materialize",
                    source_dir="source",
                    dataset_parameters={},
                    dataset_seed=0,
                    output_dir="output",
                ),
                request("shutdown"),
            ],
        )
        self.assertEqual(replies[1]["error"]["code"], "internal_error")

    def test_registration_rejects_invalid_callbacks_before_serving(self):
        for registered in [
            {},
            {"prepare": 3, "execute": lambda state, context: {}},
            callbacks(unknown=lambda: None),
        ]:
            with self.subTest(registered=registered), self.assertRaises(TypeError):
                exchange(registered, "implementation", [hello()])
        with self.assertRaisesRegex(RuntimeError, "binding API"):
            serve_bindings(
                api_version=2,
                role="implementation",
                callbacks=callbacks(),
                metadata=METADATA,
            )


if __name__ == "__main__":
    unittest.main()
