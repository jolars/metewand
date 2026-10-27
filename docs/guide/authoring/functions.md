# 2. Write the functions

Create `benchmark.py` beside the [manifest](index.md). Add the following
sections to that one file. Each role runs in a separate worker process, even
though the functions share a source file.

## Generate shared data

```python
import math
import random

from metewand import Evaluation


def generate(context):
    parameters = context.parameters
    rng = random.Random(context.seed)
    n, p = parameters["n_samples"], parameters["n_features"]
    x = [[rng.gauss(0, 1) for _ in range(p)] for _ in range(n)]
    y = [
        sum((j + 1) * row[j] for j in range(p))
        + parameters["noise"] * rng.gauss(0, 1)
        for row in x
    ]
    return {"x": x, "y": y}
```

`context.parameters` contains resolved dataset settings, including defaults.
`context.seed` is a seed derived by Metewand. Use it to initialize your own
random generator; Metewand does not change a language's random state for you.

The returned object becomes the shared JSON dataset. Both methods receive the
same generated values through their own verified copies.

## Implement the methods

```python
def gd(data, context):
    x, y = data["x"], data["y"]
    n, p = len(x), len(x[0])
    beta = [0.0] * p
    squared_norm = sum(value * value for row in x for value in row)
    if squared_norm == 0:
        return {"beta": beta}
    # The squared Frobenius norm bounds the largest eigenvalue of X'X.
    step = n / squared_norm
    for _ in range(context.parameters["iterations"]):
        residual = [
            sum(a * b for a, b in zip(row, beta)) - target
            for row, target in zip(x, y)
        ]
        gradient = [
            sum(row[j] * r for row, r in zip(x, residual)) / n
            for j in range(p)
        ]
        beta = [b - step * g for b, g in zip(beta, gradient)]
    return {"beta": beta}


def cd(data, context):
    x, y = data["x"], data["y"]
    p = len(x[0])
    beta = [0.0] * p
    residual = [-target for target in y]
    norms = [sum(row[j] ** 2 for row in x) for j in range(p)]
    for _ in range(context.parameters["iterations"]):
        for j in range(p):
            if norms[j] == 0:
                continue
            delta = -sum(row[j] * r for row, r in zip(x, residual)) / norms[j]
            beta[j] += delta
            residual = [r + row[j] * delta for row, r in zip(x, residual)]
    return {"beta": beta}
```

Here `context.parameters` contains implementation settings. Each function
returns the `beta` field declared by the problem. The SDK writes the result
envelope and checks that its values can be represented as canonical JSON.

One GD iteration is a full gradient update. One CD iteration is a full cyclic
coordinate sweep. Equal iteration limits are convenient controls, not a claim
of equal work.

## Evaluate the answers independently

```python
def evaluate(data, result, context):
    x, y, beta = data["x"], data["y"], result["beta"]
    n, p = len(x), len(x[0])
    if len(beta) != p or not all(math.isfinite(b) for b in beta):
        return Evaluation({}, "invalid")
    residual = [
        sum(a * b for a, b in zip(row, beta)) - target
        for row, target in zip(x, y)
    ]
    metrics = {
        "objective": sum(r * r for r in residual) / (2 * n),
        "gradient_inf": max(
            abs(sum(row[j] * r for row, r in zip(x, residual)) / n)
            for j in range(p)
        ),
    }
    target = context.semantics.get("one_shot_completion", {}).get("gradient_inf")
    if target is None:
        return metrics
    return Evaluation(
        metrics,
        "reached" if metrics["gradient_inf"] <= target else "not_reached",
    )
```

The evaluator recomputes metrics from the input and coefficient vector. It
does not trust a solver's own objective or convergence flag. It also rejects
vectors with the wrong dimension, which the result's array declaration alone
cannot detect.

Returning plain metrics is sufficient when the problem declares no scientific
rules. `context.semantics` supplies any declared rules; the last few lines will
apply the accuracy target we add in [part 4](validation.md).

## Check the complete benchmark

```sh
metewand check
metewand plan
metewand check --workers
metewand run --output-dir first-run
```

The plan has two candidates: one dataset and problem configuration, paired
with each method. Worker checking verifies imports and callables. This version
has no reference cases, so that check does not yet validate the metric formulas.

The run records objective and gradient metrics for both methods. Completion is
`not_required` because we have not declared an accuracy target.

Next, [vary parameters and add repetitions](parameters.md).
