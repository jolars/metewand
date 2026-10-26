"""Check the reused evaluator against a solver-independent analytic solution."""

import json
from pathlib import Path
from statistics import NormalDist
import unittest
from unittest.mock import patch

import yaml

from evaluator import assess
from solver import Solver

ROOT = Path(__file__).resolve().parent


def read(name):
    return json.loads((ROOT / "reference" / name).read_text())


class ScienceTests(unittest.TestCase):
    def setUp(self):
        self.manifest = yaml.safe_load((ROOT / "metewand.yaml").read_text())
        problem = self.manifest["problems"]["slope"]
        self.parameters = {key: value["default"]
                           for key, value in problem["parameters"].items()}
        self.semantics = problem["semantics"]
        self.data, self.result = read("data.json"), read("result.json")

    def test_both_upstream_adapters_solve_the_analytic_case(self):
        for method, module in (("pgd", "python_pgd"), ("admm", "admm")):
            with self.subTest(method=method):
                parameters = {key: field["default"] for key, field in
                              self.manifest["implementations"][method]["parameters"].items()}
                adapter = Solver(module)
                adapter.prepare(ROOT / "reference", "", self.parameters, parameters, 0)
                output = adapter.execute(None, None)
                evaluated = assess(self.data, adapter.result(output),
                                   self.parameters, self.semantics)
                self.assertEqual(evaluated.verdict, "reached")
                self.assertEqual(adapter.statistics(output)["iterations"],
                                 parameters["iterations"])

    def test_reference_has_analytic_pooled_coefficients_and_kkt_certificate(self):
        y = self.data["y"]
        n = len(y)
        self.assertEqual(self.data["x"], [[int(i == j) for j in range(n)]
                                         for i in range(n)])
        raw = [NormalDist().inv_cdf(1 - j * self.parameters["q"] / (2 * n))
               for j in range(1, n + 1)]
        scale = max(sum(y[:k]) / (n * sum(raw[:k])) for k in range(1, n + 1))
        weights = [self.parameters["reg"] * scale * weight for weight in raw]
        shifted = [value - n * weight for value, weight in zip(y, weights)]
        self.assertLess(shifted[0], shifted[1])
        pooled = (shifted[0] + shifted[1]) / 2
        expected = [0, pooled, pooled, shifted[2]]
        for actual, target in zip(self.result["beta"], expected):
            self.assertAlmostEqual(actual, target, places=14)
        coefficients = self.result["beta"][1:]
        subgradient = [(value - beta) / n for value, beta in zip(y, coefficients)]
        self.assertAlmostEqual(sum(subgradient[:2]), sum(weights[:2]), places=14)
        for value in subgradient[:2]:
            self.assertGreaterEqual(value, weights[1])
            self.assertLessEqual(value, weights[0])
        self.assertAlmostEqual(subgradient[2], weights[2], places=14)
        objective = sum((a - b) ** 2 for a, b in zip(y, coefficients)) / (2 * n)
        objective += sum(a * b for a, b in zip(weights, coefficients))
        self.assertAlmostEqual(objective, read("metrics.json")["value"], places=12)

    def test_reference_metrics_match_and_target_is_consumed(self):
        evaluated = assess(self.data, self.result, self.parameters, self.semantics)
        self.assertEqual(evaluated.verdict, "reached")
        self.assertEqual(evaluated.metrics, read("metrics.json"))
        zero = {"beta": [0] * len(self.result["beta"])}
        self.assertEqual(assess(self.data, zero, self.parameters,
                                self.semantics).verdict, "not_reached")
        self.semantics["one_shot_completion"]["rel_duality_gap"] = 2
        self.assertEqual(assess(self.data, zero, self.parameters,
                                self.semantics).verdict, "reached")

    def test_bad_dimensions_and_undeclared_intercept_are_invalid(self):
        for beta in ([0, 1], [1, *self.result["beta"][1:]], [0, float("nan"), 0, 0]):
            evaluated = assess(self.data, {"beta": beta}, self.parameters, self.semantics)
            self.assertEqual(evaluated.verdict, "invalid")
            self.assertEqual(evaluated.metrics, {})

    def test_rounding_does_not_decide_completion_or_hide_invalid_certificate(self):
        target = self.semantics["one_shot_completion"]["rel_duality_gap"]
        values = {"value": 1, "duality_gap": target, "rel_duality_gap": target + 1e-13}
        with patch("evaluator.Objective.evaluate_result", return_value=values):
            evaluated = assess(self.data, self.result, self.parameters, self.semantics)
        self.assertEqual(evaluated.metrics["rel_duality_gap"], target)
        self.assertEqual(evaluated.verdict, "not_reached")
        values["rel_duality_gap"] = 2 * self.semantics["validity"]["relative_gap_floor"]
        with patch("evaluator.Objective.evaluate_result", return_value=values):
            evaluated = assess(self.data, self.result, self.parameters, self.semantics)
        self.assertEqual(evaluated.verdict, "invalid")


if __name__ == "__main__":
    unittest.main()
