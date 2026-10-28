"""Independent checks for the two fixed qualification problems."""

import math
import sys

def prox_three(z, l1, fusion):
    order = sorted(range(3), key=z.__getitem__)
    candidates = []
    # Exhaust the four ordered partitions instead of sharing the native PAVA.
    for cuts in range(4):
        groups, group = [], []
        for rank, index in enumerate(order):
            group.append(index)
            if rank == 2 or cuts & (1 << rank):
                groups.append(group)
                group = []
        beta, levels, start = [0.0] * 3, [], 0
        for group in groups:
            end = start + len(group) - 1
            center = math.fsum(z[j] for j in group) / len(group) - fusion * (start + end - 2)
            level = math.copysign(max(0.0, abs(center) - l1), center)
            levels.append(level)
            for j in group:
                beta[j] = level
            start = end + 1
        if levels != sorted(levels):
            continue
        value = math.fsum((b - v) ** 2 for b, v in zip(beta, z)) / 2
        value += l1 * math.fsum(map(abs, beta))
        value += fusion * math.fsum(abs(beta[i] - beta[j]) for i in range(3) for j in range(i + 1, 3))
        candidates.append((value, beta))
    return min(candidates)[1]


def metrics(kind, result):
    if kind == "basin":
        x = result["x"]
        if len(x) != 2 or not all(math.isfinite(v) for v in x):
            raise ValueError("Expected two finite parameters.")
        objective = (1 - x[0]) ** 2 + 100 * (x[1] - x[0] ** 2) ** 2
        return {"objective": objective, "parameter_error": max(abs(v - 1) for v in x)}
    beta = result["coefficients"]
    if len(beta) != 3 or not all(math.isfinite(v) for v in beta) or result["intercept"] != 0:
        raise ValueError("Expected three finite coefficients and no intercept.")
    diagonal, y = [1, 2, 3], [-1.5, 1.05, 19 / 6]
    gradient = [d * (d * b - v) for d, b, v in zip(diagonal, beta, y)]
    mapped = prox_three([b - g / 9 for b, g in zip(beta, gradient)], 0.1 / 9, 0.2 / 9)
    epsilon_g = 9 * max(abs(b - m) for b, m in zip(beta, mapped)) / 9.5
    objective = math.fsum((d * b - v) ** 2 for d, b, v in zip(diagonal, beta, y)) / 2
    objective += 0.1 * math.fsum(map(abs, beta))
    objective += 0.2 * math.fsum(abs(beta[i] - beta[j]) for i in range(3) for j in range(i + 1, 3))
    return {"epsilon_g": epsilon_g, "relative_objective_error": abs(objective - 1.190138888888889) / 1.190138888888889, "coefficient_error": max(abs(b - e) for b, e in zip(beta, [-1, 0.5, 1]))}


def evaluate(context):
    from metewand import Evaluation, read_json

    measured = metrics(sys.argv[1], read_json(context.result_dir / "result.json")["data"])
    reached = measured["objective"] <= 1e-10 if sys.argv[1] == "basin" else measured["epsilon_g"] <= 1e-8 and measured["relative_objective_error"] <= 1e-8
    return Evaluation(measured, "reached" if reached else "not_reached")


if __name__ == "__main__":
    from metewand import serve_evaluator

    serve_evaluator(evaluate)
