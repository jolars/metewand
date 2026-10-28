"""A generated dataset, sum implementation, and independent evaluator."""

import math
import sys
from metewand import Evaluation, read_json, serve_dataset_materializer, serve_evaluator, serve_implementation


def materialize(context):
    return {"values": context.parameters["values"], "seed": context.seed}


class Sum:
    def prepare(self, context):
        problem_parameters = context.problem_parameters
        self.values = read_json(context.dataset_dir / "data.json")["values"]
        self.offset = problem_parameters["offset"]
        return self

    def execute(self, state, context):
        self.answer = sum(self.values) + self.offset
        return {"answer": self.answer}

    def statistics(self, output, context):
        return {"input_count": len(self.values)}

    def reset(self, state, context):
        self.__dict__.clear()


def evaluate(context):
    values = read_json(context.dataset_dir / "data.json")["values"]
    answer = read_json(context.result_dir / "result.json")["data"]["answer"]
    error = abs(answer - (sum(values) + context.problem_parameters["offset"]))
    if context.semantics["validity"]["finite_integer"] and not math.isfinite(answer):
        return Evaluation({"absolute_error": error}, "invalid")
    target = context.semantics["one_shot_completion"]["absolute_error"]
    return Evaluation({"absolute_error": error}, "reached" if error <= target else "not_reached")


if __name__ == "__main__":
    role = sys.argv[1]
    print(f"{role} stdout", flush=True)
    print(f"{role} stderr", file=sys.stderr, flush=True)
    if role == "implementation":
        worker = Sum()
        callbacks = dict(prepare=worker.prepare, execute=worker.execute,
                         statistics=worker.statistics, reset=worker.reset)
        if hasattr(worker, "check_applicability"):
            callbacks["check_applicability"] = worker.check_applicability
        serve_implementation(**callbacks)
    elif role == "dataset_materializer":
        serve_dataset_materializer(materialize)
    elif role == "problem_evaluator":
        serve_evaluator(evaluate)
    else:
        raise ValueError(f"Unknown worker role: {role}.")
