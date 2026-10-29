# Overview

Metewand runs computational benchmarks across languages. You define the
comparison in YAML, write methods and an independent evaluator in your chosen
language, and get recorded results with timings, metrics, and provenance.

It fits computations that produce an answer you can check: an estimated model,
an optimizer's solution, a simulation output, or a numerical approximation.

## What goes into a benchmark?

A benchmark has three scientific components:

- A **dataset** loads or generates the input shared by the methods.
- A **problem** defines what an answer means, what makes it valid, and how to
  evaluate it.
- An **implementation** computes an answer to that problem.

An **experiment** chooses the dataset, problem, and implementations, supplies
their parameters, and requests repetitions. Execution policies control timing
and worker behavior. Observation policies choose between a final result and a
profile of progress.

For example, a least-squares benchmark can generate a matrix and response,
compare gradient descent with coordinate descent, and independently compute
each answer's objective and gradient norm. Both methods solve the same problem;
the evaluator applies the same checks to both.

## What do you write?

Start with two files: `metewand.yaml` and `benchmark.py` or `benchmark.R`.
The manifest declares parameters, result and metric fields, and the comparison.
Ordinary functions generate data, run methods, and compute metrics. Use the
Metewand Python or R package as the entry point for your benchmark code. Each
package handles the worker protocol and writes standard result files.

You can mix worker languages, split the code into modules, use native
executables, and add more detailed contracts as needed. Each worker runs in its
own process. Metewand does not require a shared host language.

## What does a run record?

Each measurement retains effective settings, independently evaluated metrics,
timings, and an outcome. Failed attempts stay visible alongside successful
ones. A method can produce a valid answer and still miss an accuracy target.
Without a declared target, successful execution makes no convergence claim.

The default wall time includes preparation, solving, and result serialization.
It excludes worker startup, shared dataset generation, and independent
evaluation. Other timing scopes require an explicit policy and problem support.

Runs are stored in a directory you choose. You can inspect them without the
original manifest, resume unfinished work, and export JSONL or CSV for analysis.
Metewand retains individual measurements rather than choosing an aggregation
or drawing statistical conclusions.

## Choose a starting point

Follow [getting started](getting-started.md) to install the CLI and run a small
benchmark. Then work through [authoring a benchmark](authoring/index.md), which
builds a least-squares comparison in several parts:

1. Declare the comparison.
2. Write the data generator, methods, and evaluator.
3. Vary parameters and request repetitions.
4. Add scientific validation.
5. Run, inspect, and analyze the results.
6. Extend the benchmark with other languages, artifacts, or profiles.

The [examples](examples.md) provide runnable alternatives. The
[design](../design.md) explains the general model.

Metewand is under active development. The current runner executes trusted local
code and records development-environment evidence, or verifies prepared Nix
environments with `run --locked`. Isolation and the Julia package are planned.
Pin a Metewand revision when retaining benchmark runs.
