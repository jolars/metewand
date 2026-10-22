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
      text: Explore the reference
      link: /reference/

features:
  - title: Use your own language
    details: Run implementations as separate processes. Use the Python or R SDK, or implement the worker protocol in another language.
    link: /guide/examples
  - title: Define the comparison
    details: Keep datasets, problem definitions, implementations, and execution policies explicit. Evaluate results with an independent worker.
    link: /reference/problem-contracts
  - title: Keep the evidence
    details: Record content-addressed identities, execution measurements, and provenance. Inspect durable runs and export results for analysis.
    link: /reference/exports
---

## From a manifest to an inspectable run

A benchmark combines a dataset, a problem, and one or more implementations.
Metewand checks their contracts, expands the configurations into a deterministic
plan, and runs workers through a versioned process protocol.

```sh
metewand check --manifest benchmark/metewand.toml
metewand plan --manifest benchmark/metewand.toml
metewand run --manifest benchmark/metewand.toml --output-dir /tmp/metewand-run
```

The benchmark author defines what counts as a valid result and how to measure
its quality. Metewand records the execution and its guarantees so you can trace
an observation back to the inputs and policies that produced it.

## Start with a small example

The [getting started guide](guide/getting-started.md) runs a complete benchmark
from the repository. The [examples](guide/examples.md) cover Python and R
workers, generated datasets, dataset-free problems, and parameter matrices.

Metewand is under active development. The current runtime supports trusted
local, one-shot benchmarks. See the [execution reference](reference/run-cli.md)
for supported behavior and the
[roadmap](https://github.com/jolars/metewand/blob/main/TODO.md) for planned
capabilities.
