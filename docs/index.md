---
layout: home

hero:
  name: Metewand
  image:
    src: /brand/logo.png
    alt: A measuring rod with gold ends and evenly spaced graduations.
  text: |
    Reproducible benchmarks.
    Across languages.
  tagline: Compare computational methods with explicit contracts, independent
    evaluation, and provenance for every result.
  actions:
    - theme: brand
      text: Get started
      link: /guide/getting-started
    - theme: alt
      text: Overview
      link: /guide/overview

features:
  - title: Use your own language
    details: Run implementations as separate processes. Use the Python or R SDK, or implement the worker protocol in another language.
    link: /guide/authoring/extending
  - title: Define the comparison
    details: Keep datasets, problem definitions, implementations, and execution policies explicit. Evaluate results with an independent worker.
    link: /guide/authoring/
  - title: Keep the evidence
    details: Record content-addressed identities, execution measurements, and provenance. Inspect durable runs and export results for analysis.
    link: /guide/authoring/running
---

## From a manifest to an inspectable run

A benchmark combines a dataset, a problem, and one or more implementations.
Metewand checks their contracts, expands the configurations into a deterministic
plan, and runs workers through a versioned process protocol.

```sh
metewand check --manifest benchmark/metewand.yaml
metewand plan --manifest benchmark/metewand.yaml
metewand run --manifest benchmark/metewand.yaml --output-dir /tmp/metewand-run
```

The benchmark author defines what counts as a valid result and how to measure
its quality. Metewand records the execution and its guarantees so you can trace
an observation back to the inputs and policies that produced it.

## Start with a small example

The [overview](guide/overview.md) introduces the model, and
[getting started](guide/getting-started.md) runs your first benchmark.
The six-part [authoring guide](guide/authoring/index.md) builds a least-squares
comparison, then adds parameters, scientific checks, and repeated measurements.
The [examples](guide/examples.md) provide complete alternatives in Python, R,
and native Rust.

Metewand is under active development. The current runtime supports trusted
local benchmarks, including one-shot runs, fresh sequences, and buffered
profiles. See [running and analyzing comparisons](guide/authoring/running.md)
for the current controls and the
[roadmap](https://github.com/jolars/metewand/blob/main/TODO.md) for planned
capabilities.
