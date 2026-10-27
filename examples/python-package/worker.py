"""A generated dataset, sum implementation, and independent evaluator."""

import math
import sys

from metewand import (
    Evaluation,
    OneShotImplementation,
    file_entry,
    read_json,
    serve_dataset_materializer,
    serve_evaluator,
    serve_implementation,
    write_dataset_manifest,
    write_json,
)


def materialize(source_dir, dataset_parameters, seed, output_dir):
    write_json(
        output_dir / "data.json", {"values": dataset_parameters["values"], "seed": seed}
    )
    return write_dataset_manifest(
        output_dir, [file_entry(output_dir, "data.json", "application/json")]
    )


class Sum(OneShotImplementation):
    def prepare(
        self,
        dataset_instance,
        problem_contract,
        problem_parameters,
        implementation_parameters,
        seed,
    ):
        self.values = read_json(dataset_instance / "data.json")["values"]
        self.offset = problem_parameters["offset"]

    def execute(self, budget, observation_control):
        self.answer = sum(self.values) + self.offset
        return {"answer": self.answer}

    def statistics(self, output):
        return {"input_count": len(self.values)}

    def reset(self):
        self.__dict__.clear()


def evaluate(
    dataset_instance,
    problem_contract,
    problem_parameters,
    problem_semantics,
    result_dir,
    metrics_path,
):
    values = read_json(dataset_instance / "data.json")["values"]
    answer = read_json(result_dir / "result.json")["data"]["answer"]
    error = abs(answer - (sum(values) + problem_parameters["offset"]))
    if problem_semantics["validity"]["finite_integer"] and not math.isfinite(answer):
        return Evaluation({"absolute_error": error}, "invalid")
    target = problem_semantics["one_shot_completion"]["absolute_error"]
    return Evaluation({"absolute_error": error},
                      "reached" if error <= target else "not_reached")


if __name__ == "__main__":
    role = sys.argv[1]
    print(f"{role} stdout", flush=True)
    print(f"{role} stderr", file=sys.stderr, flush=True)
    if role == "implementation":
        serve_implementation(Sum())
    elif role == "dataset_materializer":
        serve_dataset_materializer(materialize)
    elif role == "problem_evaluator":
        serve_evaluator(evaluate, semantics=True)
    else:
        raise ValueError(f"Unknown worker role: {role}.")
