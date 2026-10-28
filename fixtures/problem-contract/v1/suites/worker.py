"""Benchmark-owned reference computations and independent scientific evaluators."""

import sys

from metewand import (
    Evaluation,
    read_json,
    serve_evaluator,
    serve_implementation,
)

ROLE, FAMILY, MODE = sys.argv[1:]


class Reference:
    def prepare(self, context):
        dataset, problem = context.dataset_dir, context.problem_parameters
        self.values = read_json(dataset / "data.json")["values"]
        self.parameters = problem
        return self

    def execute(self, state, context):
        if FAMILY == "sum":
            self.result = {
                "answer": sum(self.values) + self.parameters["offset"],
                "count": len(self.values),
            }
            numeric, semantic = "answer", "count"
        else:
            index = min(range(len(self.values)), key=self.values.__getitem__)
            self.result = {"index": index, "value": self.values[index]}
            numeric, semantic = "value", "index"
        if MODE == "missing_field":
            del self.result[semantic]
        elif MODE == "wrong_type":
            self.result[numeric] = "wrong"
        elif MODE == "numerically_wrong":
            self.result[numeric] += 1
        elif MODE == "semantically_invalid":
            self.result[semantic] += 1

        return self.result


def evaluate(context):
    dataset, parameters, result = context.dataset_dir, context.problem_parameters, context.result_dir
    values = read_json(dataset / "data.json")["values"]
    candidate = read_json(result / "result.json")["data"]
    if FAMILY == "sum":
        # The evaluator uses an explicit accumulation independently of the solver.
        expected = parameters["offset"]
        for value in values:
            expected += value
        valid = candidate["count"] == len(values)
        error = abs(candidate["answer"] - expected)
    else:
        expected = sorted(values)[0]
        first = next(i for i, value in enumerate(values) if value == expected)
        valid = candidate["index"] == first
        error = abs(candidate["value"] - expected)
    verdict = "invalid" if not valid else "reached" if error == 0 else "not_reached"
    return Evaluation({"absolute_error": error, "semantically_valid": valid}, verdict)



if ROLE == "implementation":
    worker = Reference()
    serve_implementation(prepare=worker.prepare, execute=worker.execute)
else:
    serve_evaluator(evaluate)
