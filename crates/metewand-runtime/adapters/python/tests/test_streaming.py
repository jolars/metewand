import tempfile
import unittest
from pathlib import Path

from metewand_adapter._streaming import StreamingContext
from metewand_adapter._worker import WorkerError


class StreamingTests(unittest.TestCase):
    def test_checkpoint_byte_limit_excludes_draft_only_metadata(self):
        for limit in [45, 46, 50]:
            with self.subTest(limit=limit), tempfile.TemporaryDirectory() as directory:
                frames = []

                def exchange(frame):
                    frames.append(frame)
                    return {"id": "1", "method": "stream_decision", "sequence": 0,
                            "action": "continue"}

                stream = StreamingContext(
                    {"policy": {"max-observations": 1, "max-checkpoint-bytes": limit},
                     "check_schedule": {"every": 1}}, Path(directory), "1", exchange)
                if limit < 46:
                    with self.assertRaises(WorkerError) as raised:
                        stream.checkpoint({"check_index": 0}, lambda: {"x": 1})
                    self.assertEqual(raised.exception.code, "resource_limit")
                    self.assertEqual(frames, [])
                else:
                    self.assertFalse(stream.checkpoint({"check_index": 0}, lambda: {"x": 1}).stop)
                    self.assertEqual(len(frames), 1)
                    self.assertEqual((Path(directory) / frames[0]["draft"]).stat().st_size, 62)

    def test_checkpoint_blocks_for_correlated_decision_and_never_serializes_skipped_checks(self):
        frames = []
        def exchange(frame):
            self.assertTrue((root / frame["draft"]).is_file())
            frames.append(frame)
            return {"id": "1", "method": "stream_decision", "sequence": 0,
                    "action": "stop", "reason": "target_reached"}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            stream = StreamingContext({"policy": {"max-observations": 3, "max-checkpoint-bytes": 65536},
                                       "check_schedule": {"every": 2, "start": 1}}, root, "1", exchange)
            def skipped():
                self.fail("unscheduled result must not be serialized")
            self.assertFalse(stream.checkpoint({"check_index": 0}, skipped).stop)
            self.assertTrue(stream.checkpoint({"check_index": 1}, lambda: {"x": 1}).stop)
            self.assertEqual(len(frames), 1)
            with self.assertRaises(WorkerError):
                stream.checkpoint({"check_index": 3}, lambda: {"x": 0})

    def test_mismatched_decision_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            stream = StreamingContext({"policy": {"max-observations": 3, "max-checkpoint-bytes": 65536},
                                       "check_schedule": {"every": 1}}, Path(directory), "1",
                                      lambda frame: {"id":"2","method":"stream_decision","sequence":0,"action":"continue"})
            with self.assertRaises(WorkerError):
                stream.checkpoint({"check_index":0}, lambda: {"x":1})
