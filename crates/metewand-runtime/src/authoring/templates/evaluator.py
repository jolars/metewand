from metewand import Evaluation, WorkerError, read_json, serve_evaluator


def evaluate(dataset_instance, problem_contract, problem_parameters,
             problem_semantics, result_dir, metrics_path):
    result = read_json(result_dir / "result.json")["data"]
    # TODO: Compute independent metrics from the dataset, parameters, and result.
    # Read validity and completion targets from problem_semantics. Return
    # Evaluation(metrics, verdict), choosing reached, not_reached, or invalid.
    raise WorkerError("operation_failed", "TODO: Implement independent metrics and a verdict.")


if __name__ == "__main__":
    serve_evaluator(evaluate, semantics=True)
