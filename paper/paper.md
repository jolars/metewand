---
title: "Metewand: Reproducible Computational Benchmarks Across Languages"
tags:
  - Rust
  - Python
  - R
  - benchmarking
  - reproducible research
  - scientific computing
authors:
  - name: Johan Larsson
    orcid: 0000-0002-4029-5945
    affiliation: "1"
affiliations:
  - name: Department of Mathematical Sciences, University of Copenhagen, Denmark
    index: 1
    ror: 035b05819
date: 17 November 2026
bibliography: paper.bib
---

# Summary

Metewand is a [command-line tool](https://github.com/jolars/metewand) for
comparing computational methods written in different programming languages.
Researchers describe a comparison in a configuration file, supply methods that
compute answers, and provide a separate evaluator that checks those answers.
Metewand runs the methods as separate processes and retains their timings,
evaluated results, failures, and the information needed to identify the data,
code, and settings used. This supports comparisons in which speed and answer
quality both matter, such as numerical optimization and statistical estimation.
The runner is written in Rust, with Python and R packages for authoring
benchmarks and a versioned protocol for native executables. It runs locally on
Linux, macOS, and Windows x64. Documentation and runnable examples are available
at [metewand.org](https://metewand.org).

# Statement of Need

Computational benchmarks inform both algorithm development and the choice of
software for research. A useful comparison must establish that methods receive
the same inputs, solve the same scientific problem, and meet comparable accuracy
requirements. Measuring elapsed time alone cannot establish this: a method may
return an inaccurate answer quickly, reach a different stopping target, or fail
on configurations where another method succeeds. Analyses such as performance
profiles [@dolan2002] depend on consistently defined measurements across test
problems.

Researchers often assemble these comparisons from scripts that combine data
generation, solver calls, timing, validation, and reporting. Extending such a
comparison to another language introduces additional interfaces and environment
management. Interrupted experiments and retries also need bookkeeping if
completed work and failures are to remain interpretable.

Metewand serves researchers developing numerical and statistical methods who
need reusable comparisons across software implementations. It separates the
scientific definition of a problem from its implementations, checks answers
through a common evaluator, and preserves individual measurements with their
provenance. Authors retain control over scientific metrics and completion
criteria, while the runner handles execution and evidence collection.

# State of the Field

Existing tools address complementary benchmarking needs. hyperfine automates
repeated command execution, warm-ups, and statistical summaries of performance
measurements [@hyperfine]. airspeed velocity tracks runtime, memory use, and
custom measurements across the history of Python packages [@asv]. IOHprofiler
provides experimentation and performance analysis for iterative optimization
heuristics, including comparisons at fixed targets and fixed budgets
[@doerr2018].

Benchopt is the closest conceptual comparison. It organizes optimization
benchmarks around datasets, objectives, and solvers, and supports algorithms
written in multiple languages through a Python framework [@moreau2022]. Metewand
shares that scientific separation but makes processes and versioned artifacts
the common interface for every worker role, including data generation and
evaluation. Its scope includes any bounded computation that produces a checkable
answer, without requiring an optimization objective.

The contribution is a shared execution and evidence model that combines
independent scientific validation, explicit measurement boundaries, stable
identities, and recovery across worker languages. These contracts motivate a
separate runner: they can remain independent of a host language's object model
while giving every implementation the same validation and persistence rules.
Existing scientific code can still be reused, as demonstrated by the Benchopt
SLOPE integration described below. Statistical analysis and publication plotting
remain downstream of Metewand's exported records.

# Software Design

A YAML manifest separates datasets, problems, implementations, and experiments.
Dataset parameters describe the input, problem parameters describe the requested
computation, and implementation parameters describe the method. Execution and
observation policies control measurement and progress recording separately. This
distinction prevents an algorithm setting, such as an iteration limit, from
implicitly becoming a scientific accuracy requirement. Parameter grids and named
cases expand into a plan that authors can inspect before running code.

Workers communicate through a versioned protocol and exchange results as
validated artifacts. Separate processes let authors combine language runtimes
and native programs without a shared foreign-function interface. Python and R
packages expose ordinary functions and lifecycle callbacks, leaving protocol
handling to the runner's adapters. Process and artifact boundaries add overhead,
so their inclusion in a measurement must be explicit.

The problem's evaluator independently computes metrics from each returned
result. Schema validation checks the result's structure, while scientific checks
determine whether it is valid and whether it reaches a declared target. A valid
answer can miss that target and still retain its metrics. Without an explicit
target, successful execution makes no convergence claim. Analytic reference
cases let authors check evaluators against independently justified answers
before measuring implementations.

Timing policies distinguish worker startup, method preparation, execution, and
result serialization. The default wall-time measurement includes preparation,
execution, and serialization, while excluding worker startup, shared dataset
construction, and independent evaluation. Problems can authorize other scopes,
including execution and serialization after untimed preparation. Optional
callback timing is labeled separately from the runner's outer wall time.
Profiles can record independently evaluated results from fresh runs at
successive budgets or from snapshots of one instrumented trajectory. The records
distinguish the clocks and instrumentation used in these comparisons.

Canonical data representations and content hashes give scientific configurations
and resolved inputs stable identities. Dataset, implementation, and scheduling
seeds have separate deterministic derivations, with paired implementation seeds
within comparisons. Benchmark code remains responsible for initializing its
random generators. Scheduling can randomize implementation order within
comparison blocks reproducibly.

The local run store combines transactional SQLite coordination with immutable
artifacts. Attempts and validated observations are distinct records, allowing
failed execution, incomplete scientific results, and accepted measurements to
remain distinguishable. Resuming a run preserves accepted work. Explicit retries
create new attempts and retain earlier evidence. Portable result archives
support offline inspection and export without the original checkout.

Optional locked execution verifies prepared Nix or uv environments. Docker OCI
and Apptainer images are also supported on Linux. These backends record
different kinds of environment evidence. Current execution runs trusted code and
does not establish complete network or filesystem isolation. Recorded identities
and environments support reproduction, but hardware and system conditions still
affect timings.

# Example

The [least-squares
example](https://github.com/jolars/metewand/tree/main/examples/least-squares)
compares gradient descent and cyclic coordinate descent on a shared simulated
dataset. After declaring the generator, methods, and evaluator, the experiment
is specified as:

```yaml
experiments:
  - name: comparison
    problem: least_squares
    datasets: [simulated]
    implementations: [gradient_descent, coordinate_descent]
    seed: 7
```

The evaluator independently computes the objective and gradient norm. A
validated variant adds analytic reference cases, an accuracy target, and cases
limited to one iteration that produce valid answers without reaching the target.
Authors inspect the configuration with `metewand plan`, run it with
`metewand run`, and export individual measurements as JSONL or CSV. Retained
measurements can be inspected with `metewand status` and exported without
rerunning the methods.

# Research Impact Statement

Initial integration examples exercise numerical optimization and penalized
regression. The [adopter
adapters](https://github.com/jolars/metewand/tree/main/examples/adopters)
compare Basin and Gomez on the Rosenbrock problem, record native Basin
trajectories, and compare proximal-gradient and hybrid methods from
clustlasso-cd. A further comparison combines a Python worker with an R
fused-lasso implementation. These examples include independent evaluators and
selected result snapshots. A retained qualification report records interruption
and recovery checks for all three comparisons, including preservation of
completed work, explicit retries, and exports that retain the same evidence in
JSONL and CSV.

The [Benchopt SLOPE
example](https://github.com/jolars/metewand/tree/main/examples/benchopt-slope)
compares solvers for regression with a penalty on sorted absolute coefficients.
It reuses an existing benchmark's dataset generator, proximal-gradient and
alternating direction method of multipliers (ADMM) solvers, and objective
evaluator without changing their scientific code. Two iteration budgets and
three measurement repetitions produce twelve attempts. Six reach the accuracy
target, while six deliberately limited attempts miss it. All twelve retain valid
observations and metrics. This provides a concrete example of migrating an
existing comparison while preserving its scientific definitions and exposing
unsuccessful outcomes.

These author-maintained integrations supply benchmark definitions and validation
procedures for larger studies. Python and R packages, a native Rust worker
example, versioned schemas, shared conformance fixtures, and regression tests
support adaptation to other numerical methods and research software.

# AI Usage Disclosure

OpenAI Codex assisted with drafting and editing this manuscript and assembling
its bibliography. Technical statements were checked against the repository's
design documentation, manifests, examples, and retained qualification records.
References were checked against project documentation and original publication
records.

# Acknowledgements

The SLOPE integration reuses scientific code from the Benchopt community's
`benchmark_slope` project.

# References
