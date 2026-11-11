# Authoring a benchmark

In this tutorial, you will run a small least-squares benchmark, change its
settings, and check the answers against an accuracy target. You will use the
existing example so you can learn how the pieces fit together before writing
your own benchmark.

The example compares gradient descent with coordinate descent. Both methods
fit a linear model to the same simulated data and return coefficients called
`beta`. You do not need to understand either algorithm to follow along.

## Open the example

First, [install the CLI and Python package](../getting-started.md#install).
Keep the Python environment with the Metewand package active. This example
needs no other Python packages.

If you already have a Metewand checkout, open its `examples/least-squares`
directory. Otherwise, get the example with Git:

```sh
git clone https://github.com/jolars/metewand.git
cd metewand/examples/least-squares
```

Run all commands below from that directory. The two main files are
`metewand.yaml`, which describes the comparison, and `benchmark.py`, which
contains the data generator, methods, and evaluator. A second manifest,
`validated.yaml`, adds accuracy checks; we will use it later.

## Read the comparison

Open `metewand.yaml`. Its four main sections connect the pieces of the benchmark:

| Section | What it does in this example |
| --- | --- |
| `datasets` | Generates 100 observations with three features and some noise. |
| `problems` | Declares the coefficient vector and the metrics used to evaluate it. |
| `implementations` | Selects the two methods and gives each 200 iterations. |
| `experiments` | Brings those choices together and sets a random seed. |

At the bottom, you will find:

```yaml
experiments:
  - name: comparison
    problem: least_squares
    datasets: [simulated]
    implementations: [gradient_descent, coordinate_descent]
    seed: 7
```

This asks Metewand to run both methods on the same generated input. The seed
makes the input repeatable. Each method runs once by default.

The [manifest walkthrough](manifest.md) explains the full file when you are
ready to change more of it.

## See how the code connects

Open `benchmark.py` and find `gd`:

```python
def gd(data, context):
    return {"beta": gradient_descent(data["x"], data["y"], context.parameters["iterations"])}
```

The manifest's `function: benchmark.py:gd` points to this function. Metewand
passes in the generated data and a context containing the method's settings.
The function returns the `beta` field declared by the problem.

The `generate` function creates the input. The `cd` function calls the other
method. Finally, `evaluate` independently computes two metrics from each
answer: `objective`, the least-squares loss, and `gradient_inf`, the largest
absolute gradient component. Smaller values indicate a better fit and a point
closer to a stationary solution, respectively.

These roles run in separate processes; Metewand supplies the data and collects
the results. You can keep ordinary solver code behind small functions like
`gd`. See [write the functions](functions.md) for the implementation details.

## Check and run it

Before running the methods, check the files and inspect the plan:

```sh
metewand check
metewand plan
metewand check --workers
```

`check` validates the manifest. `plan` shows two candidates, one for each
method. Neither runs your Python code. `check --workers` then launches workers
to check imports and function names.

Now run the comparison:

```sh
metewand run --output-dir first-run
```

Look for both method names, their timings, and the `objective` and
`gradient_inf` metrics. Completion is `not_required`: this manifest records
accuracy metrics but has not set a target that either method must reach.

The default time includes preparation, solving, and result serialization.
Generating the shared data and evaluating the answers happen outside that
time. This tiny example helps you learn the workflow; use larger problems and
repeated measurements before comparing performance.

## Make one change

Try repeating each measurement three times. In `metewand.yaml`, add
`measurement-repetitions` to the existing experiment:

```yaml
experiments:
  - name: comparison
    problem: least_squares
    datasets: [simulated]
    implementations: [gradient_descent, coordinate_descent]
    measurement-repetitions: 3
    seed: 7
```

Check the plan again, then write the new results to a new directory:

```sh
metewand plan
metewand run --output-dir repeated-run
```

There are still two method configurations, now with three measured attempts
each. Repetitions let you see timing variation without changing the scientific
question. For parameter sweeps and comparisons across input sizes, continue to
[parameters and repetitions](parameters.md).

## Add an accuracy target

The supplied `validated.yaml` uses the same Python functions and adds a target:
`gradient_inf` must be at most `1e-8`. It also includes reference cases with
known answers to check the evaluator.

```sh
metewand check --manifest validated.yaml --workers
metewand run --manifest validated.yaml --output-dir validated-run
metewand status --output-dir validated-run
```

This run includes the original 200-iteration settings and a deliberately limited
case with only one iteration per method. Expect two accepted attempts and two
completion misses. The run exits with a failure status because the limited
methods miss the target; their answers and metrics remain recorded.

This is a useful distinction: an answer can have the right shape and be valid,
yet still be insufficiently accurate. See [scientific validation](validation.md)
for how the evaluator makes that decision and how to add your own references.

## Keep and analyze the results

Inspect and export your repeated run:

```sh
metewand status --output-dir repeated-run
metewand export --output-dir repeated-run --format csv --destination tables
```

Open `tables/results.csv` in your analysis tool. Compare accuracy alongside
timing, and retain individual repetitions when assessing variation. Keep the
run directory as the original evidence; the exported table is a view of it.

Destination parents must exist, and export destinations must be new directories.
If a run is interrupted, repeat the command with the same manifest and output
directory:

```sh
metewand run --output-dir repeated-run
```

Metewand preserves matching accepted measurements and restarts interrupted
attempts automatically. See [run and analyze](running.md) for
retrying failures, choosing timing scopes, and preparing reproducible environments.

To build your own comparison, replace the generator and methods, update the
result fields, and write an evaluator that checks your scientific question.
Start with one small input again. [Extend the benchmark](extending.md) covers
other languages, artifacts, and progress profiles when you need them.
