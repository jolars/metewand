"""Bounded local observers; scientific formulas remain adapter-owned."""

import hashlib
import time
from dataclasses import dataclass
from pathlib import Path

from ._artifacts import _relative_path
from ._canonical import MAX_SAFE_INTEGER, canonical_bytes, parse_json


@dataclass(frozen=True)
class Decision:
    reason: str | None = None

    @property
    def stop(self):
        return self.reason is not None

    def __bool__(self):
        return not self.stop


def _fail(message, code="operation_failed"):
    from ._worker import WorkerError
    raise WorkerError(code, message)


def _file(path, data):
    return {"path": path, "sha256": hashlib.sha256(data).hexdigest(), "size_bytes": len(data)}


class BufferedTrace:
    """Freeze selected results and finish the checked terminal iterate before return.

    ``clock`` returns cumulative monotonic nanoseconds from the declared scope's
    origin. Native integrations can write to ``directory`` and adopt the same
    artifact contract after their solve, with no per-point Python callbacks.
    """

    def __init__(self, profile, directory, *, clock=None):
        self.profile = parse_json(canonical_bytes(profile))
        self.policy = self.profile["policy"]
        self.directory = Path(directory)
        self.points = []
        self.snapshots = []
        self.data = []
        self.reached = []
        self.reason = None
        self.finished = False
        self.adopted = None
        self.total = 0
        self.trace_bytes = 0
        self.snapshot_bytes = 0
        for key in ("max-observations", "max-trace-points", "max-buffer-bytes", "terminal-reserve-bytes"):
            value = self.policy[key]
            if type(value) is not int or not 0 < value <= MAX_SAFE_INTEGER:
                _fail(f"Invalid profile bound: {key}")
        if self.policy["terminal-reserve-bytes"] > self.policy["max-buffer-bytes"]:
            _fail("Terminal reserve exceeds buffer capacity.")
        self.schedule = self.profile["check_schedule"]
        self.schedule.setdefault("start", 0)
        if type(self.schedule["every"]) is not int or not 0 < self.schedule["every"] <= MAX_SAFE_INTEGER:
            _fail("Check interval must be a positive safe integer.")
        self.thresholds = self.policy.get("record-thresholds") or ([self.policy["threshold"]] if "threshold" in self.policy else [])
        expression = self.policy["max-active-time"].strip()
        for suffix, multiplier in (("ms", 1_000_000), ("s", 1_000_000_000), ("min", 60_000_000_000), ("h", 3_600_000_000_000)):
            if expression.endswith(suffix):
                self.active_ns = int(expression[:-len(suffix)].strip()) * multiplier
                break
        else:
            _fail("Invalid max-active-time.")
        if not 0 < self.active_ns <= MAX_SAFE_INTEGER:
            _fail("Invalid max-active-time.")
        epoch = time.perf_counter_ns()
        self.clock = clock if clock is not None else lambda: time.perf_counter_ns() - epoch

    def _reaches(self, value, threshold):
        return value <= threshold if self.policy["direction"] == "at_most" else value >= threshold

    def observe(self, progress, measure, snapshot):
        return self._check(progress, measure, snapshot, False)

    def record(self, progress, metrics, snapshot):
        return self._check(progress, lambda: metrics, snapshot, False)

    def _snapshot(self, snapshot):
        data = snapshot()
        if type(data) is not dict:
            _fail("Snapshot data must be a JSON object.")
        return canonical_bytes({"schema_version": 1, "data": data, "files": []})

    def _check(self, progress, measure, snapshot, terminal):
        if self.finished or self.reason or self.adopted:
            _fail("Recording after stop or finish violates the buffered contract.")
        if set(progress) - {"check_index", "work"}:
            _fail("Unknown progress field.")
        index = progress["check_index"]
        work = progress.get("work", {})
        if type(index) is not int or not 0 <= index <= MAX_SAFE_INTEGER or type(work) is not dict or any(type(v) is not int or not 0 <= v <= MAX_SAFE_INTEGER for v in work.values()):
            _fail("Progress must contain nonnegative safe integers.")
        previous = self.points[-1] if self.points else None
        expected = previous["check_index"] + self.schedule["every"] if previous else self.schedule["start"]
        if previous and (index <= previous["check_index"] or any(work.get(k, -1) < v for k, v in previous["work"].items())):
            _fail("Progress and work counts must be monotonic.")
        if index > expected:
            _fail("A scheduled check was skipped.")
        expired = self.clock() >= self.active_ns
        if not terminal and not expired and index != expected:
            return Decision()
        metrics = measure()
        if type(metrics) is not dict:
            _fail("Provisional metrics must be a JSON object.")
        metrics = parse_json(canonical_bytes(metrics))
        elapsed = self.clock()
        if type(elapsed) is not int or not 0 <= elapsed <= MAX_SAFE_INTEGER or (previous and elapsed < previous["instrumented_time_ns"]):
            _fail("Instrumented clock must be monotonic safe nanoseconds.")
        metric = self.policy.get("metric")
        value = metrics.get(metric) if metric else None
        if metric and (type(value) not in (int, float)):
            _fail("Provisional target metric must be finite and numeric.")
        crossed = [t for t in self.thresholds if t not in self.reached and self._reaches(value, t)]
        reason = ("target_reached" if metric and self._reaches(value, self.policy["threshold"]) else
                  "implementation_stopped" if terminal else
                  "active_time_limit" if elapsed >= self.active_ns else
                  "trace_limit" if len(self.points) + 1 >= self.policy["max-trace-points"] else None)
        capture = not metric or bool(crossed) or reason is not None
        if capture and len(self.snapshots) + 1 >= self.policy["max-observations"] and reason is None:
            reason = "observation_limit" if metric else "fixed_count"
        point = {"check_index": index, "work": parse_json(canonical_bytes(work)), "provisional_metrics": metrics,
                 "instrumented_time_ns": elapsed, "terminal": reason is not None}
        extra = len(canonical_bytes(point)) + 1
        regular = self.policy["max-buffer-bytes"] - self.policy["terminal-reserve-bytes"]
        if reason is None and self._estimate(extra, None) > regular:
            reason = "byte_limit"
            point["terminal"] = True
            extra = len(canonical_bytes(point)) + 1
        data = self._snapshot(snapshot) if capture or reason else None
        if reason is None and self._estimate(extra, data) > regular:
            reason = "byte_limit"
            point["terminal"] = True
            extra = len(canonical_bytes(point)) + 1
        if self._estimate(extra, data) > self.policy["max-buffer-bytes"]:
            _fail("Terminal check and snapshot exceed max-buffer-bytes.", "resource_limit")
        self.trace_bytes += extra
        self.points.append(point)
        if data is not None:
            self._capture(data, crossed)
        self.reached.extend(crossed)
        self.reason = reason
        return Decision(reason)

    def _capture(self, data, thresholds):
        ordinal = len(self.snapshots)
        if ordinal >= self.policy["max-observations"]:
            _fail("Terminal snapshot exceeds max-observations.", "resource_limit")
        self.snapshots.append({"capture_ordinal": ordinal, "trace_index": len(self.points) - 1,
                               "thresholds": thresholds, "manifest": f"snapshots/{ordinal}/result.json"})
        self.data.append(data)
        self.snapshot_bytes += len(data)

    def _manifest(self, total, reason, files):
        return {"schema_version": 1, "complete": True, "clock": "monotonic_ns",
                "instrumented_scope": self.profile["instrumented_scope"], "instrumented_total_time_ns": total,
                "stop_reason": reason, "trace": "trace.jsonl", "snapshots": self.snapshots.copy(), "files": files}

    def _estimate(self, extra, data):
        files = [{"path": "trace.jsonl", "sha256": "0" * 64, "size_bytes": self.trace_bytes + extra}]
        files.extend({"path": s["manifest"], "sha256": "0" * 64, "size_bytes": len(b)} for s, b in zip(self.snapshots, self.data))
        manifest = self._manifest(MAX_SAFE_INTEGER, "implementation_stopped", files)
        if data is not None:
            ordinal = len(self.snapshots)
            path = f"snapshots/{ordinal}/result.json"
            manifest["snapshots"].append({"capture_ordinal": ordinal, "trace_index": len(self.points), "thresholds": self.thresholds, "manifest": path})
            files.append({"path": path, "sha256": "0" * 64, "size_bytes": len(data)})
        return self.trace_bytes + extra + self.snapshot_bytes + (len(data) if data is not None else 0) + len(canonical_bytes(manifest))

    def finish(self, progress, measure, snapshot):
        if self.finished:
            _fail("Buffer was already finished.")
        if self.points and progress["check_index"] == self.points[-1]["check_index"]:
            if progress.get("work", {}) != self.points[-1]["work"]:
                _fail("Terminal work changed without advancing progress.")
            if not self.snapshots or self.snapshots[-1]["trace_index"] != len(self.points) - 1:
                data = self._snapshot(snapshot)
                if self._estimate(0, data) > self.policy["max-buffer-bytes"]:
                    _fail("Terminal snapshot does not fit.", "resource_limit")
                self._capture(data, [])
            self.points[-1]["terminal"] = True
        else:
            self._check(progress, measure, snapshot, True)
        self.reason = self.reason or "implementation_stopped"
        self.total = self.clock()
        if type(self.total) is not int or not self.points[-1]["instrumented_time_ns"] <= self.total <= MAX_SAFE_INTEGER:
            _fail("Invalid total instrumented duration.")
        self.finished = True

    def adopt_buffer(self, manifest="buffer.json"):
        if self.points or self.finished:
            _fail("Cannot combine a native buffer with another trajectory.")
        manifest = _relative_path(manifest)
        self.adopted = manifest
        self.finished = True

    def write(self):
        if not self.finished:
            _fail("execute_buffered must finish the terminal check before returning.")
        if self.adopted:
            return self.adopted
        self.directory.mkdir(parents=True, exist_ok=True)
        trace = b"".join(canonical_bytes(p) + b"\n" for p in self.points)
        (self.directory / "trace.jsonl").write_bytes(trace)
        files = [_file("trace.jsonl", trace)]
        for saved, data in zip(self.snapshots, self.data):
            path = self.directory / saved["manifest"]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            files.append(_file(saved["manifest"], data))
        manifest = canonical_bytes(self._manifest(self.total, self.reason, files))
        if len(trace) + self.snapshot_bytes + len(manifest) > self.policy["max-buffer-bytes"]:
            _fail("Completed buffer exceeds byte bound.", "resource_limit")
        (self.directory / "buffer.json").write_bytes(manifest)
        return "buffer.json"
