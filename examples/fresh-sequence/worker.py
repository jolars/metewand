"""Adapt a black-box function to clean, independently evaluated executions."""

import os
from metewand import OneShotImplementation, serve_implementation
from solver import bisect


class Bisection(OneShotImplementation):
    def prepare(self, dataset, contract, problem, parameters, seed):
        self.seed = seed
        self.calls = 0

    def execute(self, budget, observation_control):
        self.calls += 1
        assert self.calls == 1, "prepare must establish clean algorithmic state"
        self.control = observation_control
        return {"x": bisect(observation_control["iterations"])}

    def statistics(self, output):
        return {"pid": os.getpid(), "seed": self.seed, "control": self.control}

    def reset(self):
        self.__dict__.clear()


if __name__ == "__main__":
    serve_implementation(Bisection(), fresh_sequence=True)
