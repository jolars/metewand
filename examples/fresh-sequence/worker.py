"""Adapt a black-box function to clean, independently evaluated executions."""

import os
from metewand import serve_implementation
from solver import bisect


class Bisection:
    def prepare(self, context):
        seed = context.seed
        self.seed = seed
        self.calls = 0
        return self

    def execute(self, state, context):
        observation_control = context.observation_control
        self.calls += 1
        assert self.calls == 1, "prepare must establish clean algorithmic state"
        self.control = observation_control
        return {"x": bisect(observation_control["iterations"])}

    def statistics(self, output, context):
        return {"pid": os.getpid(), "seed": self.seed, "control": self.control}

    def reset(self, state, context):
        self.__dict__.clear()


if __name__ == "__main__":
    worker = Bisection()
    serve_implementation(prepare=worker.prepare, execute=worker.execute,
                         statistics=worker.statistics, reset=worker.reset)
