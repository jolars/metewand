import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from metewand import Artifact, stream_callback
from metewand.arrays import read_array, write_array


class StreamHelperTests(unittest.TestCase):
    def test_arrays_are_optional_and_missing_dependency_is_actionable(self):
        with (
            patch.dict(sys.modules, {"numpy": None}),
            self.assertRaisesRegex(ImportError, r"metewand\[arrays\]"),
        ):
            read_array(".", "values.npy")

    def test_natural_completion_finishes_and_stop_does_not(self):
        for stop in (False, True):
            calls = []
            stream = SimpleNamespace(
                checkpoint=lambda progress, snapshot, calls=calls, stop=stop: (
                    calls.append(progress) or SimpleNamespace(stop=stop)
                ),
                finish=lambda progress, snapshot, calls=calls: calls.append(progress),
            )
            with stream_callback(stream, dict, work_field="iterations") as callback:
                self.assertEqual(callback(), not stop)
            self.assertEqual(len(calls), 1 if stop else 2)
            self.assertEqual(calls[0], {"check_index": 0, "work": {"iterations": 0}})

    def test_failure_does_not_publish_terminal_snapshot(self):
        stream = SimpleNamespace(finish=lambda *args: self.fail("unexpected finish"))
        with (
            self.assertRaisesRegex(ValueError, "solver failed"),
            stream_callback(stream, dict),
        ):
            raise ValueError("solver failed")


@unittest.skipUnless(
    importlib.util.find_spec("numpy") and importlib.util.find_spec("scipy"),
    "optional array dependencies",
)
class ArrayHelperTests(unittest.TestCase):
    def test_dense_and_sparse_round_trip(self):
        import numpy as np
        from scipy import sparse

        with tempfile.TemporaryDirectory() as directory:
            for value in (np.arange(6).reshape(2, 3), sparse.csc_matrix(np.eye(3))):
                artifact = write_array(directory, "values", value)
                self.assertIsInstance(artifact, Artifact)
                restored = read_array(directory, artifact)
                if sparse.issparse(value):
                    self.assertEqual(restored.format, "csc")
                    np.testing.assert_array_equal(restored.toarray(), value.toarray())
                else:
                    np.testing.assert_array_equal(restored, value)

    def test_paths_and_object_arrays_are_rejected(self):
        import numpy as np

        with tempfile.TemporaryDirectory() as directory:
            for name in ("../outside", "/absolute", "a/b", "a\\b"):
                with self.assertRaises(ValueError):
                    write_array(directory, name, np.zeros(1))
            with self.assertRaises(ValueError):
                write_array(directory, "objects", np.array([object()], dtype=object))
            self.assertFalse(list(Path(directory).iterdir()))


if __name__ == "__main__":
    unittest.main()
