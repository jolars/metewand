"""Scientific acceptance cases, independent of the CLI transport."""

import json
import unittest
from pathlib import Path

from evaluator import evaluate, objective
from solver import solve

ROOT = Path(__file__).parent


class ScientificAcceptance(unittest.TestCase):
    def test_references_and_inaccurate_finite_results(self):
        for name in ("intercepts", "slope", "clustered-lasso"):
            with self.subTest(name=name):
                data = json.loads((ROOT / name / "data.json").read_text())
                reference = json.loads(
                    (ROOT / name / "reference/result.json").read_text()
                )
                metrics = json.loads(
                    (ROOT / name / "reference/metrics.json").read_text()
                )
                actual, verdict = evaluate(data, reference)
                self.assertEqual(verdict, "reached")
                for key in metrics:
                    self.assertAlmostEqual(actual[key], metrics[key], places=12)
                actual, verdict = evaluate(data, solve(data))
                self.assertEqual(verdict, "reached")
                wrong = {
                    "coefficients": [0.0] * len(reference["coefficients"]),
                    "intercept": 0.0,
                }
                self.assertEqual(evaluate(data, wrong)[1], "not_reached")
                for invalid in ([], [float("nan")] * len(reference["coefficients"])):
                    with self.assertRaises(ValueError):
                        evaluate(data, {"coefficients": invalid, "intercept": 0.0})

    def test_intercept_reference_stationarity(self):
        data = json.loads((ROOT / "intercepts/data.json").read_text())
        beta = [1.14, -0.7 + 1.92 / 18, 0.16]
        residual = [
            sum(a * b for a, b in zip(row, beta)) + 1.3 - y
            for row, y in zip(data["x"], data["y"])
        ]
        self.assertAlmostEqual(sum(residual), 0.0)
        for j, sign in enumerate([1, -1, 1]):
            self.assertAlmostEqual(
                sum(row[j] * r for row, r in zip(data["x"], residual)), -1.92 * sign
            )

    def test_slope_pooled_reference_certificate(self):
        # The two tied coefficients admit the convex combination of rank weights.
        beta, y = [1.65, 1.65, 0.25], [4, 3.8, 1]
        subgradient = [(a - b) / 3 for a, b in zip(y, beta)]
        self.assertAlmostEqual(sum(subgradient[:2]), 1.5)
        self.assertTrue(0.5 <= subgradient[0] <= 1.0)
        self.assertTrue(0.5 <= subgradient[1] <= 1.0)
        self.assertAlmostEqual(subgradient[2], 0.25)

    def test_clustered_reference_certificate_and_pair_count(self):
        # All coefficients differ, so each fusion subgradient is fixed.
        beta, y, diagonal = [-1.0, 0.5, 1.0], [-1.5, 1.05, 19 / 6], [1.0, 2.0, 3.0]
        for b, target, x, fusion, l1 in zip(
            beta, y, diagonal, [-2.0, 0.0, 2.0], [-1.0, 1.0, 1.0]
        ):
            self.assertAlmostEqual(x * (x * b - target) + 0.2 * fusion + 0.1 * l1, 0.0)
        data = json.loads((ROOT / "clustered-lasso/data.json").read_text())
        self.assertAlmostEqual(
            objective(data, beta, 0.0), 0.5 * (0.25 + 0.0025 + 1 / 36) + 1.05
        )


if __name__ == "__main__":
    unittest.main()
