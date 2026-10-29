"""Cooperative result snapshots with a blocking orchestrator decision."""

from pathlib import Path

import metewand as binding
from ._buffered import Decision, _fail
from ._canonical import MAX_SAFE_INTEGER, canonical_bytes, parse_json


class StreamingContext:
    def __init__(self, profile, directory, request_id, exchange):
        self.profile = parse_json(canonical_bytes(profile))
        self.policy = self.profile["policy"]
        self.directory = Path(directory)
        self.request_id = request_id
        self.exchange = exchange
        self.count = 0
        self.last = None
        self.stopped = None
        schedule = self.profile["check_schedule"]
        self.every = schedule["every"]
        self.start = schedule.get("start", 0)
        for value, minimum in [(self.every, 1), (self.start, 0),
                               (self.policy["max-observations"], 1),
                               (self.policy["max-checkpoint-bytes"], 1)]:
            if type(value) is not int or not minimum <= value <= MAX_SAFE_INTEGER:
                _fail("Streaming bounds and schedule must use safe integers.")

    @property
    def output_dir(self):
        return self.directory / "checkpoints" / str(self.count)

    def checkpoint(self, progress, snapshot):
        return self._capture(progress, snapshot, terminal=False)

    def finish(self, progress, snapshot):
        if self.stopped is not None or self.last == progress["check_index"]:
            return
        self._capture(progress, snapshot, terminal=True)

    def _capture(self, progress, snapshot, terminal):
        from ._bindings import _draft

        if self.stopped is not None:
            _fail("Checkpoint after stop.")
        index = progress["check_index"]
        work = progress.get("work", {})
        if type(work) is not dict or any(type(v) is not int or not 0 <= v <= MAX_SAFE_INTEGER
                                         for v in [index, *work.values()]):
            _fail("Checkpoint progress must use safe integers.")
        if self.last is not None and index <= self.last:
            _fail("Checkpoint progress must increase.")
        if not terminal and (index < self.start or (index - self.start) % self.every):
            return Decision()
        if self.count >= self.policy["max-observations"]:
            _fail("Checkpoint count exceeds the policy.")
        directory = self.output_dir
        directory.mkdir(parents=True)
        value = snapshot() if callable(snapshot) else snapshot
        if isinstance(value, binding.Result):
            draft = _draft(directory, "result", value.data, value.files)
        else:
            draft = _draft(directory, "result", value)
        # Finalization removes the draft's kind field before adding artifact metadata.
        total = -len(b'"kind":"result",')
        for path in directory.rglob("*"):
            if path.is_symlink():
                _fail("Checkpoint files must not be symbolic links.")
            if path.is_file():
                total += path.stat().st_size
            if total > self.policy["max-checkpoint-bytes"]:
                _fail("Checkpoint exceeds max-checkpoint-bytes.", "resource_limit")
        frame = {"id": self.request_id, "event": "checkpoint", "sequence": self.count,
                 "check_index": index, "work": work, "terminal": terminal,
                 "draft": (directory / draft).relative_to(self.directory).as_posix()}
        decision = self.exchange(frame)
        keys = {"id", "method", "sequence", "action"}
        if (type(decision) is not dict or set(decision) not in (keys, keys | {"reason"})
                or decision.get("id") != self.request_id
                or decision.get("method") != "stream_decision"
                or type(decision.get("sequence")) is not int
                or decision["sequence"] != self.count
                or decision.get("action") not in {"continue", "stop"}):
            _fail("Decision does not match the pending checkpoint.")
        action = decision["action"]
        reason = decision.get("reason")
        if (action == "continue" and ("reason" in decision or terminal)
                or action == "stop" and reason not in {
                    "target_reached", "fixed_count", "observation_limit", "active_time_limit",
                    "implementation_stopped", "byte_limit"}):
            _fail("Invalid streaming decision or stopping reason.")
        self.last = index
        self.count += 1
        self.stopped = reason
        return Decision(reason)
