"""Independent full objectives and analytic references for the fixed smoke inputs.

This module never imports an implementation or uses implementation statistics.
The closed-form references apply only to the checked-in inputs.
"""

import math

REFERENCES = {
    "intercepts": ([1.14, -0.7 + 1.92 / 18, 0.16], 1.3),
    "slope": ([1.65, 1.65, 0.25], 0.0),
    "clustered-lasso": ([-1.0, 0.5, 1.0], 0.0),
}


def objective(data, beta, intercept):
    residuals = [
        math.fsum(a * b for a, b in zip(row, beta)) + intercept - y
        for row, y in zip(data["x"], data["y"])
    ]
    loss = math.fsum(r * r for r in residuals) / 2
    if data["family"] == "slope":
        return loss / len(residuals) + math.fsum(
            w * b for w, b in zip(data["weights"], sorted(map(abs, beta), reverse=True))
        )
    penalty = data["l1"] * math.fsum(map(abs, beta))
    if data["family"] == "clustered-lasso":
        # Explicit unordered pairs independently check the sorted prox's convention.
        penalty += data["fusion"] * math.fsum(
            abs(a - b) for i, a in enumerate(beta) for b in beta[i + 1 :]
        )
    return loss + penalty


def evaluate(data, result):
    expected, expected_intercept = REFERENCES[data["family"]]
    beta, intercept = result["coefficients"], result["intercept"]
    if len(beta) != len(expected) or not all(
        math.isfinite(v) for v in [*beta, intercept]
    ):
        raise ValueError("Coefficients must be finite and have the problem dimension.")
    value = objective(data, beta, intercept)
    metrics = {
        "objective": value,
        "objective_error": abs(value - objective(data, expected, expected_intercept)),
        "coefficient_error": max(abs(a - b) for a, b in zip(beta, expected)),
        "intercept_error": abs(intercept - expected_intercept),
    }
    reached = all(
        metrics[key] <= 1e-8
        for key in ("objective_error", "coefficient_error", "intercept_error")
    )
    return metrics, "reached" if reached else "not_reached"
