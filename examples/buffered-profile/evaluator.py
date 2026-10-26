"""Independent scientific evaluation, without importing a solver."""

from metewand import Evaluation, read_json, serve_evaluator


def evaluate(dataset, contract, parameters, result, metrics_path):
    x = read_json(result / "result.json")["data"]["x"]
    return Evaluation({"gap": x ** 2}, "reached" if x == 0 else "not_reached")


if __name__ == "__main__":
    serve_evaluator(evaluate)
