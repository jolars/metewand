# Least squares, starting small

This example compares gradient descent with cyclic coordinate descent on one
seeded, simulated 100-by-3 dataset. It minimizes
`sum((X beta - y)^2) / (2 n)`, without an intercept. The only dependency beyond
Python's standard library is the Metewand package installed in the selected Python
environment.

From this directory, with `metewand` on `PATH`:

```sh
metewand check --workers
metewand plan
metewand run --output-dir run
metewand status --output-dir run
metewand export --output-dir run --format csv --destination tables
```

The basic manifest declares parameters, result and metric shapes, function
references, and the comparison. Local execution, one final result per method,
and one measurement repetition are defaults. Both methods use 200 iterations:
one gradient update for GD and one complete cyclic sweep for CD. These controls
do not imply equal computational work.

The evaluator independently computes the objective and infinity norm of the
gradient. The basic run reports metrics with `completion: not_required`; it
does not claim convergence. The evaluator rejects coefficient vectors with the
wrong dimension. Canonical JSON validation rejects nonfinite output values.

## Add scientific checks

`validated.yaml` uses the same functions and adds scientific rules, a gradient
target of `1e-8`, and inline analytic reference cases. The references cover an
exact solution, an incomplete solution, and an invalid coefficient vector.
Its extra iteration-limit case demonstrates a valid result that misses the
completion target:

```sh
metewand check --manifest validated.yaml --workers
metewand run --manifest validated.yaml --output-dir validated-run
metewand status --output-dir validated-run
```

Expect two accepted attempts and two completion misses. All four retain their
metrics, and the run exits unsuccessfully because the limited cases miss their
target. The evaluator reads the target from its context and returns an explicit
`Evaluation` when one is supplied.

## Measured scope

The default `prepare_and_execute` wall time includes loading the shared JSON
dataset, solving, and serializing the result. Worker startup and module imports,
dataset generation, independent evaluation, and durable publication are outside
the primary interval. This small example teaches authoring and validation;
its timings include Python and orchestration costs.

Add experiment repetitions or named execution policies for reuse, warm-ups,
and other controls. Stronger environment backends remain separate roadmap
work; this example records development-environment
and observed-control evidence.
