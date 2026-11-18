import hashlib
import tempfile
import unittest
from pathlib import Path

from binding_support import exchange, hello, request
from metewand_adapter._buffered import BufferedTrace
from metewand_adapter._canonical import MAX_SAFE_INTEGER, canonical_bytes, parse_json
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
    def test_snapshot_files_count_toward_both_byte_limits(self):
        spec = profile()
        spec["policy"].update({"max-buffer-bytes": 4096, "terminal-reserve-bytes": 2048})
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trace = BufferedTrace(spec, root, clock=lambda: 100)
            progress = {"check_index": 0}
            metrics = {"gap": 0.5}

            def snapshot(size):
                return trace.snapshot(
                    {"coefficients": "arrays/beta.bin"},
                    files=[("arrays/beta.bin", "application/octet-stream", bytes([42]) * size)],
                )

            with self.assertRaisesRegex(WorkerError, "exceed max-buffer-bytes"):
                trace.record(progress, metrics, lambda: snapshot(4096))
            self.assertEqual(trace.record(progress, metrics, lambda: snapshot(2048)).reason, "byte_limit")
            trace.finish(progress, lambda: metrics, lambda: self.fail("snapshot already captured"))
            manifest = parse_json((root / trace.write()).read_bytes())
            payload = bytes([42]) * 2048
            saved = root / "snapshots/0"
            self.assertEqual((saved / "arrays/beta.bin").read_bytes(), payload)
            entry = {"path": "arrays/beta.bin", "size_bytes": 2048, "sha256": hashlib.sha256(payload).hexdigest()}
            result = parse_json((saved / "result.json").read_bytes())
            self.assertEqual(result["files"], [{**entry, "media_type": "application/octet-stream"}])
            self.assertIn({**entry, "path": "snapshots/0/arrays/beta.bin"}, manifest["files"])

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
                replies[0]["capabilities"], ["managed_outputs", "problem_preparation", "buffered_profile"]
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

    def test_many_snapshots_fit_the_canonical_manifest_budget_exactly(self):
        captures = 128
        spacing = 32
        last = captures * spacing
        thresholds = list(range(captures - 1, -1, -1))
        spec = profile()
        spec["policy"].update({
            "threshold": 0,
            "record-thresholds": thresholds,
            "max-observations": captures,
            "max-trace-points": last + 1,
            "max-buffer-bytes": 2_000_000,
        })

        def run(root, end):
            trace = BufferedTrace(spec, root, clock=lambda: 100)
            for index in range(end + 1):
                gap = captures - index // spacing
                if index % spacing >= 2:
                    gap = captures + 1
                decision = trace.record(
                    {"check_index": index}, {"gap": gap}, lambda: {"x": index}
                )
                if index < end:
                    self.assertFalse(decision.stop)
            trace.finish(
                {"check_index": end},
                lambda: self.fail("duplicate measure"),
                lambda: self.fail("duplicate snapshot"),
            )
            manifest = parse_json((root / trace.write()).read_bytes())
            return decision, manifest, (root / "trace.jsonl").read_bytes().splitlines(keepends=True)

        with tempfile.TemporaryDirectory() as directory:
            decision, manifest, rows = run(Path(directory), last)
        self.assertEqual(decision.reason, "target_reached")
        self.assertEqual(len(manifest["snapshots"]), captures)
        self.assertEqual(len(rows), last + 1)

        end = last - spacing
        prefix = {
            **manifest,
            "stop_reason": "implementation_stopped",
            "instrumented_total_time_ns": MAX_SAFE_INTEGER,
            "snapshots": manifest["snapshots"][:-2] + [
                {**manifest["snapshots"][-2], "thresholds": thresholds}
            ],
            "files": manifest["files"][:-1],
        }
        prefix["files"][0] = {
            **prefix["files"][0], "size_bytes": sum(map(len, rows[:end + 1]))
        }
        regular = len(canonical_bytes(prefix)) + sum(f["size_bytes"] for f in prefix["files"])
        for limit, reason in [(regular, None), (regular - 1, "byte_limit")]:
            with self.subTest(limit=limit), tempfile.TemporaryDirectory() as directory:
                spec["policy"]["max-buffer-bytes"] = limit + spec["policy"]["terminal-reserve-bytes"]
                decision, actual, actual_rows = run(Path(directory), end)
                self.assertEqual(decision.reason, reason)
                self.assertEqual(actual["snapshots"], manifest["snapshots"][:-1])
                terminal = parse_json(rows[end])
                terminal["terminal"] = True
                self.assertEqual(actual_rows, rows[:end] + [canonical_bytes(terminal) + b"\n"])

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
