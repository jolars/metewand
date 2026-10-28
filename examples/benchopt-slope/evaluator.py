"""Reuse the upstream objective in a process that imports no solver."""

import numpy as np

from metewand import Evaluation, read_json, serve_evaluator
from upstream.objective import Objective


def assess(data, result, parameters, semantics):
    x, y = np.asarray(data["x"]), np.asarray(data["y"])
    beta = np.asarray(result["beta"])
    if (beta.shape != (x.shape[1] + 1,) or not np.isfinite(beta).all()
            or (not parameters["fit_intercept"] and beta[0] != 0)):
        return Evaluation({}, "invalid")
    target = semantics["one_shot_completion"]["rel_duality_gap"]
    objective = Objective(**parameters, target_rel_duality_gap=target)
    objective.set_data(x, y)
    values = objective.evaluate_result(beta)
    if not all(np.isfinite(value) for value in values.values()):
        return Evaluation({}, "invalid")
    gap = values["rel_duality_gap"]
    digits = semantics["reporting"]["decimal_places"]
    metrics = {
        key: round(float(values[key]), digits)
        for key in ("value", "duality_gap", "rel_duality_gap")
    }
    # Acceptance uses the unrounded certificate, including its roundoff bound.
    if gap < semantics["validity"]["relative_gap_floor"]:
        return Evaluation(metrics, "invalid")
    return Evaluation(metrics, "reached" if gap <= target else "not_reached")


def evaluate(context):
    return assess(read_json(context.dataset_dir / "data.json"),
                  read_json(context.result_dir / "result.json")["data"],
                  context.problem_parameters, context.semantics)


if __name__ == "__main__":
    serve_evaluator(evaluate)
