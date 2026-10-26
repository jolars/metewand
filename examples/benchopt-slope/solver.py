"""Run upstream callback solvers for an explicit, fixed number of iterations."""

import importlib
import sys

import numpy as np

from metewand import OneShotImplementation, read_json, serve_implementation
from upstream.objective import Objective


class Solver(OneShotImplementation):
    def __init__(self, method):
        self.solver_type = importlib.import_module(f"upstream.{method}").Solver

    def prepare(self, dataset_instance, problem_contract, problem_parameters,
                implementation_parameters, seed):
        data = read_json(dataset_instance / "data.json")
        objective = Objective(**problem_parameters)
        objective.set_data(np.asarray(data["x"]), np.asarray(data["y"]))
        parameters = dict(implementation_parameters)
        self.iterations = parameters.pop("iterations")
        self.solver = self.solver_type.get_instance(**parameters)
        self.solver.set_objective(**objective.get_objective())

    def execute(self, budget, observation_control):
        self.completed = 0

        def callback():
            # Benchopt callbacks run before the first update and after each update.
            if self.completed == self.iterations:
                return False
            self.completed += 1
            return True

        self.solver.run(callback)
        return self.solver.get_result()

    def result(self, output):
        return {"beta": output["beta"].tolist()}

    def statistics(self, output):
        return {"iterations": self.completed}


if __name__ == "__main__":
    method = {"pgd": "python_pgd", "admm": "admm"}[sys.argv[1]]
    serve_implementation(Solver(method))
