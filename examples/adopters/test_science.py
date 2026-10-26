import unittest

from evaluator import metrics, prox_three


class IndependentEvaluation(unittest.TestCase):
    def test_fixed_references_and_inaccurate_iterates(self):
        basin = metrics("basin", {"x": [1, 1]})
        self.assertEqual(basin, {"objective": 0, "parameter_error": 0})
        self.assertAlmostEqual(metrics("basin", {"x": [-1.2, 1]})["objective"], 24.2)
        optimum = metrics("clustlasso", {"coefficients": [-1, 0.5, 1], "intercept": 0})
        self.assertLess(optimum["epsilon_g"], 1e-15)
        self.assertLess(optimum["relative_objective_error"], 1e-15)
        initial = metrics("clustlasso", {"coefficients": [0, 0, 0], "intercept": 0})
        self.assertGreater(initial["epsilon_g"], 0.1)
        self.assertGreater(initial["relative_objective_error"], 1)

    def test_prox_enumeration_handles_distinct_and_pooled_groups(self):
        for actual, expected in zip(prox_three([2, -1, 0.3], 0.2, 0.1), [1.6, -0.6, 0.1]):
            self.assertAlmostEqual(actual, expected)
        self.assertEqual(prox_three([1, 1, 1], 0.2, 10), [0.8, 0.8, 0.8])
        self.assertEqual(prox_three([0.1, -0.1, 0], 1, 1), [0, 0, 0])

    def test_malformed_outputs_fail_instead_of_becoming_accuracy_points(self):
        for kind, result in [("basin", {"x": [1]}), ("basin", {"x": [float("nan"), 1]}), ("clustlasso", {"coefficients": [0, 0], "intercept": 0}), ("clustlasso", {"coefficients": [0, 0, 0], "intercept": 1})]:
            with self.subTest(kind=kind, result=result), self.assertRaises(ValueError):
                metrics(kind, result)


if __name__ == "__main__":
    unittest.main()
