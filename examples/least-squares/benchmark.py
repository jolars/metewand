"""Least squares with two in-house solvers and independent gradient metrics."""

import math
import random

from metewand import Evaluation


def generate(context):
    parameters = context.parameters
    rng = random.Random(context.seed)
    n, p = parameters["n_samples"], parameters["n_features"]
    x = [[rng.gauss(0, 1) for _ in range(p)] for _ in range(n)]
    y = [sum((j + 1) * row[j] for j in range(p))
         + parameters["noise"] * rng.gauss(0, 1) for row in x]
    return {"x": x, "y": y}


def gradient_descent(x, y, iterations):
    """Each iteration performs one full gradient update."""
    n, p = len(x), len(x[0])
    beta = [0.0] * p
    squared_norm = sum(value * value for row in x for value in row)
    if squared_norm == 0:
        return beta
    # The squared Frobenius norm bounds the largest eigenvalue of X'X.
    step = n / squared_norm
    for _ in range(iterations):
        residual = [sum(a * b for a, b in zip(row, beta)) - target
                    for row, target in zip(x, y)]
        gradient = [sum(row[j] * r for row, r in zip(x, residual)) / n
                    for j in range(p)]
        beta = [b - step * g for b, g in zip(beta, gradient)]
    return beta


def coordinate_descent(x, y, iterations):
    """Each iteration performs one complete cyclic coordinate sweep."""
    p = len(x[0])
    beta = [0.0] * p
    residual = [-target for target in y]
    norms = [sum(row[j] ** 2 for row in x) for j in range(p)]
    for _ in range(iterations):
        for j in range(p):
            if norms[j] == 0:
                continue
            delta = -sum(row[j] * r for row, r in zip(x, residual)) / norms[j]
            beta[j] += delta
            residual = [r + row[j] * delta for row, r in zip(x, residual)]
    return beta


def gd(data, context):
    return {"beta": gradient_descent(data["x"], data["y"], context.parameters["iterations"])}


def cd(data, context):
    return {"beta": coordinate_descent(data["x"], data["y"], context.parameters["iterations"])}


def evaluate(data, result, context):
    x, y, beta = data["x"], data["y"], result["beta"]
    n, p = len(x), len(x[0])
    if len(beta) != p or not all(math.isfinite(b) for b in beta):
        return Evaluation({}, "invalid")
    residual = [sum(a * b for a, b in zip(row, beta)) - target
                for row, target in zip(x, y)]
    metrics = {
        "objective": sum(r * r for r in residual) / (2 * n),
        "gradient_inf": max(abs(sum(row[j] * r for row, r in zip(x, residual)) / n)
                            for j in range(p)),
    }
    target = context.semantics.get("one_shot_completion", {}).get("gradient_inf")
    if target is None:
        return metrics
    return Evaluation(metrics, "reached" if metrics["gradient_inf"] <= target else "not_reached")
