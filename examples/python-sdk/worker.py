"""A generated dataset, sum implementation, and independent evaluator."""

import sys

from metewand import (
    Implementation,
    file_entry,
    read_json,
    serve_dataset_materializer,
    serve_evaluator,
    serve_implementation,
    write_dataset_manifest,
    write_json,
    write_metrics,
    write_result,
)


def materialize(source_dir, dataset_parameters, seed, output_dir):
    write_json(
        output_dir / "data.json", {"values": dataset_parameters["values"], "seed": seed}
    )
    return write_dataset_manifest(
        output_dir, [file_entry(output_dir, "data.json", "application/json")]
    )


class Sum(Implementation):
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
        return {"input_count": len(self.values)}

    def write_result(self, directory):
        return write_result(directory, {"answer": self.answer})

    def reset(self):
        self.__dict__.clear()


def evaluate(
    dataset_instance, problem_contract, problem_parameters, result_dir, metrics_path
):
    values = read_json(dataset_instance / "data.json")["values"]
    answer = read_json(result_dir / "result.json")["data"]["answer"]
    error = abs(answer - (sum(values) + problem_parameters["offset"]))
    write_metrics(metrics_path, {"absolute_error": error})
    return "reached" if error == 0 else "invalid"


if __name__ == "__main__":
    role = sys.argv[1]
    print(f"{role} stdout", flush=True)
    print(f"{role} stderr", file=sys.stderr, flush=True)
    if role == "implementation":
        serve_implementation(Sum())
    elif role == "dataset_materializer":
        serve_dataset_materializer(materialize)
    elif role == "problem_evaluator":
        serve_evaluator(evaluate)
    else:
        raise ValueError(f"Unknown worker role: {role}.")
