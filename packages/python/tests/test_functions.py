import tempfile
import unittest
from pathlib import Path

from metewand import Evaluation, read_json
from metewand._functions import FunctionContext, FunctionImplementation, evaluate_function, materialize_function, read_function_data


class FunctionTests(unittest.TestCase):
    def test_data_and_results_use_standard_envelopes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            materialize_function(lambda context: {"values": [context.parameters["n"], context.seed]}, root, {"n": 3}, 7, root)
            worker = FunctionImplementation(lambda data, context: {"answer": sum(data["values"]) + context.parameters["offset"]})
            worker.prepare(root, "contract", {}, {"offset": 2}, 11)
            output = worker.execute(None, None)
            worker.write_result(root, output)
            self.assertEqual(output, {"answer": 12})
            self.assertEqual(read_json(root / "result.json")["data"], output)

    def test_plain_metrics_require_no_scientific_rules(self):
        context = FunctionContext({}, {}, None, {"validity": {}, "one_shot_completion": {}})
        outcome = evaluate_function(lambda data, result, context: {"error": 0}, {}, {}, context)
        self.assertEqual(outcome, Evaluation({"error": 0}, "not_required"))
        for group in ("validity", "one_shot_completion"):
            context.semantics[group] = {"error": 0}
            with self.assertRaisesRegex(ValueError, "explicit.*verdict"):
                evaluate_function(lambda *args: {"error": 0}, {}, {}, context)
            verdict = "reached" if group == "one_shot_completion" else "not_required"
            self.assertEqual(evaluate_function(lambda *args: Evaluation({"error": 0}, verdict), {}, {}, context).verdict, verdict)
            context.semantics[group] = {}

    def test_only_empty_unit_datasets_can_omit_data_json(self):
        with tempfile.TemporaryDirectory() as directory:
            dataset = Path(directory)
            self.assertEqual(read_function_data(dataset), {})
            (dataset / "other.json").write_text("{}")
            with self.assertRaises(FileNotFoundError):
                read_function_data(dataset)
            (dataset / "data.json").write_text("[]")
            with self.assertRaisesRegex(TypeError, "JSON object"):
                read_function_data(dataset)

    def test_missing_target_cannot_be_reported_as_convergence(self):
        context = FunctionContext({}, {}, None, {})
        with self.assertRaisesRegex(ValueError, "completion"):
            evaluate_function(lambda *args: Evaluation({}, "reached"), {}, {}, context)


if __name__ == "__main__":
    unittest.main()
