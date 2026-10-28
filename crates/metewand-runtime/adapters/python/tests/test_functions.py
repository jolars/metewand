import io
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path

from binding_support import exchange, hello, request
from metewand import write_json
from metewand_adapter._artifacts import read_json


class FunctionTests(unittest.TestCase):
    def test_function_roles_use_binding_contexts_and_managed_drafts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "benchmark.py"
            source.write_text("""
from metewand import Evaluation


def generate(context):
    return {"values": [context.parameters["n"], context.seed]}


def solve(data, context):
    return {"answer": sum(data["values"]) + context.parameters["offset"]}


def evaluate(data, result, context):
    assert context.semantics["one_shot_completion"] == {"error": 0}
    expected = sum(data["values"]) + context.parameters["offset"]
    return Evaluation({"error": result["answer"] - expected}, "reached")
""")
            dataset = root / "dataset"
            result = root / "result"
            metrics = root / "metrics"
            for path in [dataset, result, metrics]:
                path.mkdir()

            replies = exchange(
                {"function": (source, "generate")},
                "dataset_materializer",
                [
                    hello("dataset_materializer"),
                    request(
                        "materialize",
                        source_dir=directory,
                        dataset_parameters={"n": 3},
                        dataset_seed=7,
                        output_dir=str(dataset),
                    ),
                    request("shutdown"),
                ],
            )
            draft = read_json(dataset / replies[1]["draft"])
            self.assertEqual(
                draft,
                {
                    "schema_version": 1,
                    "kind": "dataset",
                    "data": {"values": [3, 7]},
                    "files": [],
                },
            )
            write_json(dataset / "data.json", draft["data"])

            replies = exchange(
                {"function": (source, "solve")},
                "implementation",
                [
                    hello(),
                    request(
                        "prepare",
                        dataset_dir=str(dataset),
                        problem_contract_id="contract",
                        problem_parameters={},
                        implementation_parameters={"offset": 2},
                        implementation_seed=11,
                    ),
                    request("execute", result_dir=str(result)),
                    request("reset"),
                    request("shutdown"),
                ],
            )
            draft = read_json(result / replies[2]["draft"])
            self.assertEqual(draft["data"], {"answer": 12})
            self.assertTrue(replies[3]["ok"])
            write_json(
                result / "result.json",
                {"schema_version": 1, "data": draft["data"], "files": []},
            )

            replies = exchange(
                {"function": (source, "evaluate")},
                "problem_evaluator",
                [
                    hello("problem_evaluator"),
                    request(
                        "evaluate",
                        dataset_dir=str(dataset),
                        problem_contract_id="contract",
                        problem_parameters={"offset": 2},
                        problem_semantics={"one_shot_completion": {"error": 0}},
                        result_dir=str(result),
                        metrics_path=str(metrics / "metrics.json"),
                    ),
                    request("shutdown"),
                ],
            )
            self.assertEqual(replies[1]["verdict"], "reached")
            self.assertEqual(
                read_json(metrics / replies[1]["draft"])["data"], {"error": 0}
            )

    def test_only_empty_unit_datasets_can_omit_data_json(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "benchmark.py"
            source.write_text("def solve(data, context):\n    return data\n")
            dataset = root / "dataset"
            dataset.mkdir()
            prepare = request(
                "prepare",
                dataset_dir=str(dataset),
                problem_contract_id="contract",
                problem_parameters={},
                implementation_parameters={},
                implementation_seed=0,
            )
            for content, valid in [
                (None, True),
                ("other.json", False),
                ("data.json", False),
            ]:
                if content:
                    (dataset / content).write_text("[]")
                with redirect_stderr(io.StringIO()):
                    replies = exchange(
                        {"function": (source, "solve")},
                        "implementation",
                        [hello(), prepare, request("shutdown")],
                    )
                self.assertEqual(replies[1]["ok"], valid)

    def test_function_import_and_selection_failures_are_reported_during_hello(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "benchmark.py"
            for content in ["raise RuntimeError('import failed')", "solve = 3"]:
                source.write_text(content)
                with redirect_stderr(io.StringIO()):
                    replies = exchange(
                        {"function": (source, "solve")},
                        "implementation",
                        [hello(), request("shutdown")],
                    )
                self.assertFalse(replies[0]["ok"])
                self.assertEqual(replies[0]["error"]["code"], "operation_failed")
