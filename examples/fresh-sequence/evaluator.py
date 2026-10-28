"""Evaluate the scientific result without importing either solver."""

from metewand import Evaluation, read_json, serve_evaluator


def evaluate(context):
    x = read_json(context.result_dir / "result.json")["data"]["x"]
    if not 1 <= x <= 2:
        return Evaluation({"residual": abs(x * x - 2)}, "invalid")
    residual = round(abs(x * x - 2), 12)
    return Evaluation({"residual": residual}, "reached" if residual == 0 else "not_reached")


if __name__ == "__main__":
    serve_evaluator(evaluate)
