import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from metewand import canonical_bytes, parse_json, write_result
from metewand._artifacts import _windows_relative_path


class PortableArtifactsTests(unittest.TestCase):
    def test_path_objects_keep_logical_forward_slashes(self):
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "nested").mkdir()
            self.assertEqual(
                write_result(directory, {}, manifest=Path("nested/result.json")),
                "nested/result.json",
            )

    def test_windows_aliases_never_reach_filesystem_operations(self):
        for value in ["C:/escape", "C:escape", "file:stream", "NUL", "aux.txt", "COM1.json", "lpt²", "trailing.", "trailing ", "bad?name", "bad\x01name"]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                _windows_relative_path(value)
        for value in ["ordinary/file.json", "computer.json", "COM10", "null.txt"]:
            _windows_relative_path(value)


@unittest.skipUnless(os.name == "nt", "Inherited native handles require Windows.")
class WindowsTransportTests(unittest.TestCase):
    def test_native_handles_preserve_binary_frames_and_stop_inheritance(self):
        import msvcrt

        input_read, input_write = os.pipe()
        output_read, output_write = os.pipe()
        self.addCleanup(os.close, input_write)
        self.addCleanup(os.close, output_read)
        handles = [msvcrt.get_osfhandle(input_read), msvcrt.get_osfhandle(output_write)]
        for handle in handles:
            os.set_handle_inheritable(handle, True)
        startup = subprocess.STARTUPINFO()
        startup.lpAttributeList = {"handle_list": handles}
        environment = dict(os.environ, METEWAND_PROTOCOL_READ_HANDLE=str(handles[0]),
                           METEWAND_PROTOCOL_WRITE_HANDLE=str(handles[1]))
        source = """
import os
from metewand import serve_dataset_materializer
from metewand._worker import _windows_protocol_stream
reader = _windows_protocol_stream('READ', 'rb')
writer = _windows_protocol_stream('WRITE', 'wb')
assert not os.get_inheritable(reader.fileno())
assert not os.get_inheritable(writer.fileno())
assert not os.get_handle_inheritable(int(os.environ['METEWAND_PROTOCOL_READ_HANDLE']))
assert not os.get_handle_inheritable(int(os.environ['METEWAND_PROTOCOL_WRITE_HANDLE']))
reader.close()
writer.close()
print('worker logs stay on stdout', flush=True)
serve_dataset_materializer(lambda *args: 'dataset.json')
"""
        try:
            process = subprocess.Popen([sys.executable, "-c", source], env=environment,
                                       startupinfo=startup, close_fds=True,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        finally:
            os.close(input_read)
            os.close(output_write)
        requests = [
            {"id": "0", "method": "hello", "protocols": [1], "role": "dataset_materializer",
             "worker_id": "mw1-resolved-worker-" + "a" * 64},
            {"id": "1", "method": "shutdown"},
        ]
        os.write(input_write, b"".join(canonical_bytes(request) + b"\n" for request in requests))
        stdout, stderr = process.communicate(timeout=15)
        self.assertEqual(process.returncode, 0, stderr.decode())
        self.assertIn(b"worker logs stay on stdout", stdout)
        payload = os.read(output_read, 4096)
        self.assertNotIn(b"\r", payload)
        responses = [parse_json(frame) for frame in payload.splitlines()]
        self.assertEqual([response["id"] for response in responses], ["0", "1"])
        self.assertTrue(all(response["ok"] for response in responses))


if __name__ == "__main__":
    unittest.main()
