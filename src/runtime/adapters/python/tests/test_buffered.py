import tempfile
import unittest
from pathlib import Path

from binding_support import exchange, hello, request
from metewand_adapter._buffered import BufferedTrace
from metewand_adapter._canonical import parse_json
from metewand_adapter._worker import WorkerError


def profile():
    return {
        "policy": {
            "metric": "gap",
            "direction": "at_most",
            "threshold": 0.01,
            "record-thresholds": [1.0, 0.1, 0.01],
            "max-observations": 3,
            "max-trace-points": 100,
            "max-buffer-bytes": 65536,
            "terminal-reserve-bytes": 8192,
            "max-active-time": "1 s",
        },
        "instrumented_scope": "solver",
        "scope": {
            "clock-origin": "solver entry",
            "included-work": "initialization, updates, and diagnostics",
            "excluded-preparation": "dataset loading",
            "metric-agreement": {},
        },
        "instrumentation": {},
        "check_schedule": {"every": 1, "start": 0},
    }


class BufferedTests(unittest.TestCase):
    def test_buffered_callback_receives_the_observer_and_assigned_directory(self):
        state = {"prepared": True}

        def execute_buffered(prepared, context):
            self.assertIs(prepared, state)
            self.assertIsNone(context.budget)
            self.assertEqual(context.output_dir, Path(directory))
            context.observer.finish(
                {"check_index": 0}, lambda: {"gap": 0.001}, lambda: {"x": 0.0}
            )

        with tempfile.TemporaryDirectory() as directory:
            replies = exchange(
                {
                    "prepare": lambda context: state,
                    "execute_buffered": execute_buffered,
                },
                "implementation",
                [
                    hello(),
                    request(
                        "prepare",
                        dataset_dir=directory,
                        problem_contract_id="contract",
                        problem_parameters={},
                        implementation_parameters={},
                        implementation_seed=0,
                    ),
                    request("execute", result_dir=directory),
                    request(
                        "execute_buffered", result_root=directory, profile=profile()
                    ),
                    request("shutdown"),
                ],
            )
            self.assertEqual(
                replies[0]["capabilities"], ["managed_outputs", "buffered_profile"]
            )
            self.assertEqual(replies[2]["error"]["code"], "unsupported_operation")
            manifest = parse_json(
                (Path(directory) / replies[3]["buffer"]["manifest"]).read_bytes()
            )
            self.assertEqual(manifest["stop_reason"], "target_reached")

    def test_frozen_snapshot_and_threshold_deduplication(self):
        with tempfile.TemporaryDirectory() as directory:
            clock = iter([10, 20, 30, 40, 50])
            trace = BufferedTrace(profile(), Path(directory), clock=lambda: next(clock))
            state = {"x": [1.0]}
            calls = []

            def snapshot():
                calls.append(1)
                return state

            self.assertEqual(
                trace.observe(
                    {"check_index": 0}, lambda: {"gap": 0.001}, snapshot
                ).reason,
                "target_reached",
            )
            state["x"][0] = 9.0
            trace.finish(
                {"check_index": 0}, lambda: self.fail("duplicate measure"), snapshot
            )
            manifest = parse_json((Path(directory) / trace.write()).read_bytes())
            self.assertEqual(len(manifest["snapshots"]), 1)
            self.assertEqual(len(calls), 1)
            saved = parse_json(
                (Path(directory) / manifest["snapshots"][0]["manifest"]).read_bytes()
            )
            self.assertEqual(saved["data"]["x"], [1.0])
            self.assertEqual(manifest["snapshots"][0]["thresholds"], [1.0, 0.1, 0.01])
            self.assertEqual(manifest["instrumented_total_time_ns"], 30)

    def test_terminal_between_checks_and_record_after_stop(self):
        with tempfile.TemporaryDirectory() as directory:
            spec = profile()
            spec["check_schedule"]["every"] = 5
            trace = BufferedTrace(spec, Path(directory), clock=lambda: 100)
            trace.record({"check_index": 0}, {"gap": 2.0}, lambda: {"x": [0.0]})
            trace.finish({"check_index": 3}, lambda: {"gap": 1.5}, lambda: {"x": [3.0]})
            manifest = parse_json((Path(directory) / trace.write()).read_bytes())
            self.assertEqual(manifest["stop_reason"], "implementation_stopped")
            self.assertEqual(manifest["snapshots"][0]["trace_index"], 1)
            with self.assertRaises(WorkerError):
                trace.record({"check_index": 4}, {"gap": 1.0}, lambda: {})

    def test_oversized_terminal_snapshot_fails_without_a_buffer(self):
        with tempfile.TemporaryDirectory() as directory:
            trace = BufferedTrace(profile(), Path(directory), clock=lambda: 100)
            with self.assertRaises(WorkerError) as caught:
                trace.finish(
                    {"check_index": 0},
                    lambda: {"gap": 10.0},
                    lambda: {"x": "x" * 70000},
                )
            self.assertEqual(caught.exception.code, "resource_limit")
            self.assertFalse((Path(directory) / "buffer.json").exists())

    def test_schedule_skips_callbacks_and_fixed_count_stops_at_the_last_capture(self):
        with tempfile.TemporaryDirectory() as directory:
            spec = profile()
            for key in ("metric", "direction", "threshold", "record-thresholds"):
                del spec["policy"][key]
            spec["check_schedule"] = {"every": 5, "start": 2}
            trace = BufferedTrace(spec, Path(directory), clock=lambda: 100)
            calls = []
            for index in range(13):
                decision = trace.observe(
                    {"check_index": index},
                    lambda: calls.append(index) or {"gap": 10.0},
                    lambda: {"x": index},
                )
                self.assertEqual(decision.stop, index == 12)
            self.assertEqual(calls, [2, 7, 12])
            self.assertEqual(decision.reason, "fixed_count")
            trace.finish(
                {"check_index": 12}, lambda: self.fail("duplicate check"), lambda: {}
            )
            manifest = parse_json((Path(directory) / trace.write()).read_bytes())
            self.assertEqual(len(manifest["snapshots"]), 3)
            self.assertEqual(manifest["stop_reason"], "fixed_count")

    def test_native_buffer_submission_does_not_replay_observations(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            native = BufferedTrace(profile(), root, clock=lambda: 100)
            native.finish(
                {"check_index": 0}, lambda: {"gap": 0.001}, lambda: {"x": 0.0}
            )
            manifest = native.write()
            saved = (root / manifest).read_bytes()
            adapter = BufferedTrace(profile(), root)
            adapter.adopt_buffer(manifest)
            self.assertEqual(adapter.write(), manifest)
            self.assertEqual((root / manifest).read_bytes(), saved)
