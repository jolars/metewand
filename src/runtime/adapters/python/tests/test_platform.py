from pathlib import Path
import tempfile
import unittest

from metewand_adapter._artifacts import _windows_relative_path, write_result


class PortableArtifactsTests(unittest.TestCase):
    def test_path_objects_keep_logical_forward_slashes(self):
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "nested").mkdir()
            self.assertEqual(
                write_result(directory, {}, manifest=Path("nested/result.json")),
                "nested/result.json",
            )

    def test_windows_aliases_never_reach_filesystem_operations(self):
        for value in [
            "C:/escape",
            "C:escape",
            "file:stream",
            "NUL",
            "aux.txt",
            "COM1.json",
            "lpt²",
            "trailing.",
            "trailing ",
            "bad?name",
            "bad\x01name",
        ]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                _windows_relative_path(value)
        for value in ["ordinary/file.json", "computer.json", "COM10", "null.txt"]:
            _windows_relative_path(value)
