# 1. Declare the comparison

This walkthrough builds a least-squares benchmark with two methods: gradient
descent and coordinate descent. You will generate shared data, evaluate both
answers independently, vary their settings, add accuracy checks, and export
the measurements.

The code uses Python's standard library and the Metewand package. Complete the
[installation](../getting-started.md#install) first and keep that Python
environment active. The finished code is also available in the
[least-squares example](https://github.com/jolars/metewand/tree/main/examples/least-squares).

## State the question

We will compare how two methods minimize

```text
sum((X beta - y)^2) / (2 n)
```

on the same simulated matrix `X` and response `y`, without an intercept. Each
method returns a coefficient vector `beta`. An independent evaluator computes
the objective and the largest absolute gradient component.

Start by recording both metrics. Later, we will use the gradient norm as an
accuracy target. The two methods both accept an iteration count, but one
gradient update and one coordinate sweep do different amounts of work.

## Create the manifest

```sh
metewand init /tmp/least-squares --empty
cd /tmp/least-squares
```

Replace `metewand.yaml` with:

```yaml
version: 1
name: least-squares

datasets:
  simulated:
    function: benchmark.py:generate
    parameters:
      n_samples: {type: integer, minimum: 1, default: 100}
      n_features: {type: integer, minimum: 1, default: 3}
      noise: {type: number, minimum: 0, default: 0.1}

problems:
  least_squares:
    result:
      beta: {type: array, items: number, minItems: 1}
    metrics:
      objective: {type: number, minimum: 0, optional: true}
      gradient_inf: {type: number, minimum: 0, optional: true}
    evaluator: {function: benchmark.py:evaluate}

implementations:
  gradient_descent:
    function: benchmark.py:gd
    parameters:
      iterations: {type: integer, minimum: 1, default: 200}
  coordinate_descent:
    function: benchmark.py:cd
    parameters:
      iterations: {type: integer, minimum: 1, default: 200}

experiments:
  - name: comparison
    problem: least_squares
    datasets: [simulated]
    implementations: [gradient_descent, coordinate_descent]
    seed: 7
```

The dataset declares the input parameters. The problem declares the answer's
shape and the metrics its evaluator can return. The implementations declare
their own settings. The experiment chooses what to compare and supplies a
seed.

`function: benchmark.py:gd` selects the `gd` function in a source file beside
the manifest. Metewand includes that file in the worker's source bundle.
There is no worker protocol code to write.

We mark the metric fields optional so a later validity check can reject an
unusable result without inventing numerical metrics for it. Required result
fields still have to exist and satisfy their declarations.

## Keep the first run small

We have omitted named execution and observation policies. This selects local
execution, one final result, and one measurement per method. The primary time
is wall time over preparation, solving, and result serialization. `plan` will
show these resolved defaults once the source file exists.

There is no accuracy target yet. The evaluator can report useful metrics
without claiming either method converged.

Next, [write the functions](functions.md) referenced by this manifest.
