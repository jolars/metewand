import io
import json
import os
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from unittest.mock import patch

from metewand import (
    CanonicalJsonError,
    Evaluation,
    Implementation,
    OneShotImplementation,
    WorkerError,
    canonical_bytes,
    file_entry,
    parse_json,
    read_json,
    serve_dataset_materializer,
    write_dataset_manifest,
    write_json,
    write_metrics,
    write_result,
)
from metewand._worker import (
    MAX_LINE_BYTES,
    ProtocolError,
    _encode_response,
    _object,
    _serve,
)

ROOT = Path(__file__).resolve().parents[3]
WORKER_ID = "mw1-resolved-worker-" + "a" * 64


def hello(role="implementation", **changes):
    return dict(
        id="0", method="hello", protocols=[1], role=role, worker_id=WORKER_ID, **changes
    )


def request(method, **fields):
    return dict(id="1", method=method, **fields)


def exchange(worker, role, requests, semantics=False):
    source = io.BytesIO(b"".join(canonical_bytes(r) + b"\n" for r in requests))
    output = io.BytesIO()
    _serve(worker, role, source, output, semantics=semantics)
    lines = output.getvalue().splitlines()
    for line in lines:
        assert canonical_bytes(parse_json(line)) == line
    return [parse_json(line) for line in lines]


class Sum(Implementation):
    def prepare(
        self,
        dataset_instance,
        problem_contract,
        problem_parameters,
        implementation_parameters,
        seed,
    ):
        self.answer = sum(read_json(dataset_instance / "data.json")["values"])
        self.answer += problem_parameters["offset"]
        self.seed = seed

    def execute(self, budget, observation_control):
        assert budget is None and observation_control is None
        self.answer += 1
        return {"seed": self.seed}

    def write_result(self, directory):
        return write_result(
            directory, {"answer": self.answer}, schema="schemas/result.json"
        )

    def reset(self):
        self.__dict__.clear()


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
            (root / "link").symlink_to(Path(outside))
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
    def test_data_callbacks_retain_custom_writers_and_consume_failed_results(self):
        class Solver(OneShotImplementation):
            def prepare(self, *args):
                pass

            def execute(self, *args):
                return {"answer": 8}

            def write_result(self, directory, output):
                write_json(directory / "answer.json", output)
                return write_result(directory, output, manifest="custom.json",
                    files=[file_entry(directory, "answer.json", "application/json")])

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            calls = [hello(), request("prepare", dataset_dir=directory,
                problem_contract_id="c", problem_parameters={},
                implementation_parameters={}, implementation_seed=0),
                request("execute", result_dir=directory), request("shutdown")]
            responses = exchange(Solver(), "implementation", calls)
            self.assertEqual(responses[2]["result"], {"manifest": "custom.json"})
            self.assertEqual(read_json(root / "answer.json"), {"answer": 8})
            self.assertEqual(read_json(root / "custom.json")["files"][0]["path"], "answer.json")

            class Invalid(Solver):
                def result(self, output):
                    return {"answer": float("nan")}

                write_result = OneShotImplementation.write_result

            with redirect_stderr(io.StringIO()):
                responses = exchange(Invalid(), "implementation", calls[:-1] + calls[-2:])
            self.assertEqual(responses[2]["error"]["code"], "operation_failed")
            self.assertEqual(responses[3]["error"]["code"], "invalid_request")

    def test_data_callbacks_write_envelopes_outside_solve_timing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            events = []

            class Solver(OneShotImplementation):
                def prepare(self, dataset, contract, problem, implementation, seed):
                    self.value = problem["offset"]

                def execute(self, budget, observation_control):
                    events.append("solve")
                    return self.value + 6

                def result(self, output):
                    events.append("convert")
                    return {"answer": output}

                def statistics(self, output):
                    events.append("statistics")
                    return {"iterations": 1}

            def clock():
                events.append("clock")
                return len(events)

            with patch("metewand._worker.time.perf_counter_ns", side_effect=clock):
                responses = exchange(Solver(), "implementation", [
                    hello(), request("prepare", dataset_dir=directory,
                        problem_contract_id="c", problem_parameters={"offset": 2},
                        implementation_parameters={}, implementation_seed=0),
                    request("execute", result_dir=directory), request("shutdown"),
                ])
            self.assertEqual(events, ["clock", "solve", "clock", "statistics", "convert"])
            self.assertEqual(responses[2]["implementation_time_ns"], 2)
            self.assertEqual(responses[2]["statistics"], {"iterations": 1})
            self.assertEqual((root / "result.json").read_bytes(),
                b'{"data":{"answer":8},"files":[],"schema_version":1}')

            def evaluate(dataset, contract, parameters, semantics, result, metrics):
                return Evaluation({"error": 1}, "not_reached")

            responses = exchange(evaluate, "problem_evaluator", [
                hello("problem_evaluator"), request("evaluate", dataset_dir=directory,
                    problem_contract_id="c", problem_parameters={}, problem_semantics={},
                    result_dir=directory, metrics_path=str(root / "assigned.json")),
                request("shutdown"),
            ], semantics=True)
            self.assertEqual(responses[1]["verdict"], "not_reached")
            self.assertEqual((root / "assigned.json").read_bytes(),
                b'{"data":{"error":1},"schema_version":1}')
            for metrics, verdict in [({}, "yes"), ([], "reached"), ({"x": float("nan")}, "invalid")]:
                with self.subTest(metrics=metrics, verdict=verdict), self.assertRaises(ValueError):
                    Evaluation(metrics, verdict)

    def test_evaluator_negotiates_scientific_rules_and_rejects_bad_shapes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            rules = {
                "preprocessing": {},
                "validity": {"finite": True},
                "one_shot_completion": {"absolute_error": 0.25},
            }

            def evaluate(dataset, contract, parameters, semantics, result, metrics):
                self.assertEqual(semantics, rules)
                write_metrics(metrics, {"error": 0.2})
                return (
                    "reached"
                    if 0.2 <= semantics["one_shot_completion"]["absolute_error"]
                    else "not_reached"
                )

            call = request(
                "evaluate",
                dataset_dir=directory,
                problem_contract_id="c",
                problem_parameters={},
                problem_semantics=rules,
                result_dir=directory,
                metrics_path=str(root / "metrics.json"),
            )
            responses = exchange(
                evaluate,
                "problem_evaluator",
                [hello("problem_evaluator"), call, request("shutdown")],
                semantics=True,
            )
            self.assertEqual(responses[0]["capabilities"], ["problem_semantics"])
            self.assertEqual(responses[1]["verdict"], "reached")
            for invalid in [None, [], True]:
                bad = dict(call, problem_semantics=invalid)
                responses = exchange(
                    evaluate,
                    "problem_evaluator",
                    [hello("problem_evaluator"), bad, request("shutdown")],
                    semantics=True,
                )
                self.assertEqual(responses[1]["error"]["code"], "invalid_request")
            responses = exchange(
                evaluate,
                "problem_evaluator",
                [hello("problem_evaluator"), call, request("shutdown")],
            )
            self.assertEqual(responses[1]["error"]["code"], "invalid_request")

    def test_implementation_lifecycle_and_callback_timing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_json(root / "data.json", {"values": [1, 2, 3]})
            events = []

            class TracedSum(Sum):
                def prepare(self, *args):
                    events.append("prepare")
                    super().prepare(*args)

                def execute(self, *args):
                    events.append("execute")
                    return super().execute(*args)

                def write_result(self, *args):
                    events.append("write_result")
                    return super().write_result(*args)

            worker = TracedSum()
            calls = [
                hello(),
                request(
                    "prepare",
                    dataset_dir=str(root),
                    problem_contract_id="contract",
                    problem_parameters={"offset": 2},
                    implementation_parameters={},
                    implementation_seed=23,
                ),
                request("execute", result_dir=str(root)),
                request("reset"),
                request("execute", result_dir=str(root)),
                request("shutdown"),
            ]
            ticks = iter([100, 125])

            def clock():
                events.append("clock")
                return next(ticks)

            def validate_statistics(value, name):
                events.append("statistics")
                return _object(value, name)

            with (
                patch("metewand._worker.time.perf_counter_ns", side_effect=clock),
                patch("metewand._worker._object", side_effect=validate_statistics),
            ):
                responses = exchange(worker, "implementation", calls)
            self.assertEqual(
                events,
                ["prepare", "clock", "execute", "clock", "statistics", "write_result"],
            )
            self.assertEqual(
                responses[0],
                {
                    "id": "0",
                    "ok": True,
                    "protocol": 1,
                    "worker_id": WORKER_ID,
                    "sdk": {"name": "metewand-python", "version": "0.1.0"},
                    "capabilities": ["one_shot"],
                },
            )
            self.assertEqual(
                responses[2],
                {
                    "id": "1",
                    "ok": True,
                    "implementation_time_ns": 25,
                    "statistics": {"seed": 23},
                    "result": {"manifest": "result.json"},
                },
            )
            self.assertEqual(read_json(root / "result.json")["data"], {"answer": 9})
            self.assertEqual(worker.__dict__, {})
            self.assertEqual(responses[4]["error"]["code"], "invalid_request")

    def test_materializer_and_evaluator_dispatch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def materialize(source_dir, parameters, seed, output_dir):
                self.assertEqual(source_dir, root)
                write_json(
                    output_dir / "data.json",
                    {"values": parameters["values"], "seed": seed},
                )
                return write_dataset_manifest(
                    output_dir,
                    [file_entry(output_dir, "data.json", "application/json")],
                )

            responses = exchange(
                materialize,
                "dataset_materializer",
                [
                    hello("dataset_materializer"),
                    request(
                        "materialize",
                        source_dir=str(root),
                        dataset_parameters={"values": [1]},
                        dataset_seed=17,
                        output_dir=str(root),
                    ),
                    request("shutdown"),
                ],
            )
            self.assertEqual(
                responses[1]["dataset"], {"manifest": "dataset-manifest.json"}
            )

            def evaluate(dataset, contract, parameters, result, metrics_path):
                self.assertEqual(
                    (dataset, result, contract, parameters), (root, root, "c", {})
                )
                write_metrics(metrics_path, {"error": 0}, schema="schemas/metrics.json")
                return "not_required"

            responses = exchange(
                evaluate,
                "problem_evaluator",
                [
                    hello("problem_evaluator"),
                    request(
                        "evaluate",
                        dataset_dir=str(root),
                        problem_contract_id="c",
                        problem_parameters={},
                        result_dir=str(root),
                        metrics_path=str(root / "metrics.json"),
                    ),
                    request("shutdown"),
                ],
            )
            self.assertEqual(
                responses[1], {"id": "1", "ok": True, "verdict": "not_required"}
            )

    def test_worker_errors_and_unexpected_callback_failures(self):
        for error, code in [
            (
                WorkerError("operation_failed", "bad data", {"index": 2}),
                "operation_failed",
            ),
            (RuntimeError("broken"), "operation_failed"),
        ]:

            def fail(*args, error=error):
                raise error

            log = io.StringIO()
            with redirect_stderr(log):
                responses = exchange(
                    fail,
                    "dataset_materializer",
                    [
                        hello("dataset_materializer"),
                        request(
                            "materialize",
                            source_dir="s",
                            dataset_parameters={},
                            dataset_seed=0,
                            output_dir="o",
                        ),
                        request("shutdown"),
                    ],
                )
            self.assertEqual(responses[1]["error"]["code"], code)
            if isinstance(error, WorkerError):
                self.assertEqual(
                    responses[1]["error"],
                    {"code": code, "message": "bad data", "details": {"index": 2}},
                )
                self.assertEqual(log.getvalue(), "")
            else:
                self.assertIn("RuntimeError: broken", log.getvalue())
                self.assertIn("RuntimeError: broken", responses[1]["error"]["message"])
                self.assertIn("Traceback", responses[1]["error"]["message"])
            self.assertTrue(responses[2]["ok"])
        with self.assertRaises(ValueError):
            WorkerError("invented", "bad")

    def test_unexpected_exception_text_is_safe_and_bounded(self):
        for message in ["bad \ud800 text", "x" * MAX_LINE_BYTES + " final detail"]:
            def fail(*args):
                raise RuntimeError(message)

            with redirect_stderr(io.StringIO()):
                responses = exchange(
                    fail, "dataset_materializer",
                    [hello("dataset_materializer"),
                     request("materialize", source_dir="s", dataset_parameters={},
                             dataset_seed=0, output_dir="o"), request("shutdown")],
                )
            error = responses[1]["error"]
            self.assertEqual(error["code"], "operation_failed")
            self.assertLess(len(canonical_bytes(error)), 65536)
            if len(message) > MAX_LINE_BYTES:
                self.assertIn("[traceback truncated]", error["message"])
                self.assertIn("final detail", error["message"])
            else:
                self.assertIn("bad \\ud800 text", error["message"])
            self.assertTrue(responses[2]["ok"])

    def test_invalid_requests_do_not_reach_callbacks(self):
        for bad in [
            request(
                "prepare",
                dataset_dir="d",
                problem_contract_id="c",
                problem_parameters={},
                implementation_parameters={},
                implementation_seed=True,
            ),
            request("execute", result_dir="r", budget={}),
            request(
                "materialize",
                source_dir="s",
                dataset_parameters={},
                dataset_seed=0,
                output_dir="o",
            ),
            request("reset", extra=True),
            request("shutdown", extra=True),
            request("execute_stream"),
            hello(),
        ]:
            with self.subTest(bad=bad):
                responses = exchange(
                    Sum(), "implementation", [hello(), bad, request("shutdown")]
                )
                self.assertFalse(responses[1]["ok"])
                self.assertIn(
                    responses[1]["error"]["code"],
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
            value = hello()
            value.update(changes)
            responses = exchange(Sum(), "implementation", [value])
            self.assertFalse(responses[0]["ok"])

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
                _serve(Sum(), "implementation", io.BytesIO(data), io.BytesIO())

    def test_missing_outputs_are_operation_failures(self):
        with redirect_stderr(io.StringIO()):
            responses = exchange(
                lambda *args: "missing.json",
                "dataset_materializer",
                [
                    hello("dataset_materializer"),
                    request(
                        "materialize",
                        source_dir="s",
                        dataset_parameters={},
                        dataset_seed=0,
                        output_dir="o",
                    ),
                    request("shutdown"),
                ],
            )
        self.assertEqual(responses[1]["error"]["code"], "operation_failed")

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
        _serve(Sum(), "implementation", io.BytesIO(source), output)
        self.assertEqual(len(output.getvalue().splitlines()), 2)

    def test_oversized_callback_failure_is_bounded(self):
        def fail(*args):
            raise WorkerError(
                "operation_failed", "large", {"text": "x" * MAX_LINE_BYTES}
            )

        responses = exchange(
            fail,
            "dataset_materializer",
            [
                hello("dataset_materializer"),
                request(
                    "materialize",
                    source_dir="s",
                    dataset_parameters={},
                    dataset_seed=0,
                    output_dir="o",
                ),
                request("shutdown"),
            ],
        )
        self.assertEqual(responses[1]["error"]["code"], "internal_error")

    def test_explicit_protocol_descriptors_remain_owned_by_caller(self):
        input_read, input_write = os.pipe()
        output_read, output_write = os.pipe()
        try:
            os.write(
                input_write,
                canonical_bytes(hello("dataset_materializer"))
                + b"\n"
                + canonical_bytes(request("shutdown"))
                + b"\n",
            )
            serve_dataset_materializer(
                lambda *args: None, read_fd=input_read, write_fd=output_write
            )
            os.fstat(input_read)
            os.fstat(output_write)
            responses = os.read(output_read, 4096).splitlines()
            self.assertEqual(len(responses), 2)
            self.assertEqual(parse_json(responses[1]), {"id": "1", "ok": True})
        finally:
            for descriptor in [input_read, input_write, output_read, output_write]:
                os.close(descriptor)

    def test_failed_prepare_and_reset_clear_prepared_state(self):
        class BrokenPrepare(Sum):
            def prepare(self, *args):
                if hasattr(self, "prepared_once"):
                    raise WorkerError("operation_failed", "Preparation failed.")
                self.prepared_once = True

            reset = Implementation.reset

        prepare = request(
            "prepare",
            dataset_dir="d",
            problem_contract_id="c",
            problem_parameters={},
            implementation_parameters={},
            implementation_seed=0,
        )
        for failing_request, code in [
            (prepare, "operation_failed"),
            (request("reset"), "unsupported_operation"),
        ]:
            responses = exchange(
                BrokenPrepare(),
                "implementation",
                [
                    hello(),
                    prepare,
                    failing_request,
                    request("execute", result_dir="r"),
                    request("shutdown"),
                ],
            )
            self.assertTrue(responses[1]["ok"])
            self.assertEqual(responses[2]["error"]["code"], code)
            self.assertEqual(responses[3]["error"]["code"], "invalid_request")

    def test_evaluator_must_write_metrics_and_supply_a_verdict(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            calls = [
                hello("problem_evaluator"),
                request(
                    "evaluate",
                    dataset_dir=str(root),
                    problem_contract_id="c",
                    problem_parameters={},
                    result_dir=str(root),
                    metrics_path=str(root / "metrics.json"),
                ),
                request("shutdown"),
            ]
            with redirect_stderr(io.StringIO()):
                responses = exchange(
                    lambda *args: "reached", "problem_evaluator", calls
                )
            self.assertEqual(responses[1]["error"]["code"], "operation_failed")
            for verdict in [
                "invalid",
                "reached",
                "not_reached",
                "not_required",
                "invented",
            ]:

                def evaluate(*args, verdict=verdict):
                    write_metrics(args[-1], {}, schema="metrics.json")
                    return verdict

                with redirect_stderr(io.StringIO()):
                    responses = exchange(evaluate, "problem_evaluator", calls)
                if verdict == "invented":
                    self.assertEqual(responses[1]["error"]["code"], "operation_failed")
                else:
                    self.assertEqual(responses[1]["verdict"], verdict)


if __name__ == "__main__":
    unittest.main()
