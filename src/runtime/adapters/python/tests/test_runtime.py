"""Explicit download caches travel separately from scientific parameters."""

import tempfile
import unittest

from binding_support import exchange, hello, request


class RuntimeTests(unittest.TestCase):
    def test_materializer_context_has_explicit_or_empty_runtime_bindings(self):
        for runtime in [None, {"version": 1, "caches": {"provider": "/downloads"}}]:
            seen = []

            def materialize(context):
                seen.append(context.runtime)
                self.assertEqual(context.parameters, {"rows": 2})
                return {"rows": context.parameters["rows"]}

            with tempfile.TemporaryDirectory() as directory:
                materialization = request(
                    "materialize", source_dir=directory, dataset_parameters={"rows": 2},
                    dataset_seed=17, output_dir=directory,
                )
                if runtime is not None:
                    materialization["runtime"] = runtime
                replies = exchange(
                    {"materialize": materialize}, "dataset_materializer",
                    [hello("dataset_materializer"), materialization, request("shutdown")],
                )
                self.assertIn("runtime_config", replies[0]["capabilities"])
                self.assertTrue(replies[1]["ok"], replies[1])
                self.assertEqual(seen, [runtime or {"version": 1, "caches": {}}])

    def test_invalid_runtime_bindings_do_not_invoke_materializers(self):
        for runtime in [
            None, {"version": True, "caches": {}}, {"version": 2, "caches": {}},
            {"version": 1, "caches": {"bad/name": "/downloads"}},
            {"version": 1, "caches": {"provider": ""}},
            {"version": 1, "caches": {"provider": "bad\0path"}},
            {"version": 1, "caches": {}, "parameters": {}},
        ]:
            replies = exchange(
                {"materialize": lambda context: self.fail("Invalid runtime reached callback.")},
                "dataset_materializer",
                [hello("dataset_materializer"), request(
                    "materialize", source_dir="source", dataset_parameters={},
                    dataset_seed=17, output_dir="output", runtime=runtime,
                ), request("shutdown")],
            )
            self.assertEqual(replies[1]["error"]["code"], "invalid_request", replies)
