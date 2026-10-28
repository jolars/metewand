import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from metewand import Artifact, Context, Dataset, Evaluation, Result, serve_implementation


class BindingTests(unittest.TestCase):
    def test_values_do_not_require_cli(self):
        self.assertEqual(Result({"answer": 1}).data, {"answer": 1})
        self.assertIsNone(Dataset(files=[Artifact("data.bin", "application/octet-stream")]).data)
        self.assertEqual(Evaluation({}, "not_required").verdict, "not_required")
        self.assertEqual(Context(parameters={}, future_field=3).future_field, 3)

    def test_unchanged_binding_forwards_new_callbacks_to_cli(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter = Path(directory, "adapter.py")
            adapter.write_text("BINDING_API_VERSION = 1\ndef serve_bindings(**kw): return kw\n")
            callback = lambda state, context: None
            with patch.dict(os.environ, {"METEWAND_ADAPTER_PATH": str(adapter)}):
                call = serve_implementation(prepare=lambda c: None, execute=callback, future_callback=callback)
            self.assertEqual(call["api_version"], 1)
            self.assertIs(call["callbacks"]["future_callback"], callback)

    def test_missing_or_incompatible_cli_adapter_is_actionable(self):
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(RuntimeError, "Metewand CLI"):
                serve_implementation()
        with tempfile.TemporaryDirectory() as directory:
            adapter = Path(directory, "adapter.py")
            adapter.write_text("BINDING_API_VERSION = 2\n")
            with patch.dict(os.environ, {"METEWAND_ADAPTER_PATH": str(adapter)}):
                with self.assertRaisesRegex(RuntimeError, "binding API"):
                    serve_implementation()
