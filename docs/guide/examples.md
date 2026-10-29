# Examples

Start with the [getting started guide](getting-started.md) for a two-file
benchmark. The [authoring walkthrough](authoring/index.md) builds a comparison
with scientific checks and repeated measurements.

These examples provide complete code and reproduction commands. Follow each
example's README for its working directory and dependencies.

| Example | What to learn |
| --- | --- |
| [Least squares](https://github.com/jolars/metewand/tree/main/examples/least-squares) | Compare ordinary functions, then add accuracy targets and analytic references. |
| [Parameter matrix](https://github.com/jolars/metewand/tree/main/examples/parameter-matrix) | Combine dataset, problem, and implementation grids with named cases. |
| [Python package](https://github.com/jolars/metewand/tree/main/examples/python-package) | Separate preparation and solving with lifecycle callbacks. |
| [R package](https://github.com/jolars/metewand/tree/main/examples/r-package) | Use the same worker roles and lifecycle in R. |
| [Native Rust](https://github.com/jolars/metewand/tree/main/examples/rust-worker) | Run a native implementation and evaluator with standard artifacts. |
| [Generated dataset](https://github.com/jolars/metewand/tree/main/examples/generated-dataset) | Materialize datasets and write custom artifacts. |
| [Fresh sequences](https://github.com/jolars/metewand/tree/main/examples/fresh-sequence) | Compare black-box solvers over clean runs with independent targets. |
| [Buffered profiles](https://github.com/jolars/metewand/tree/main/examples/buffered-profile) | Instrument a trajectory and evaluate selected snapshots. |
| [Streaming profiles](https://github.com/jolars/metewand/tree/main/examples/streaming-profile) | Pause at checkpoints for independent evaluation and durable stopping decisions. |

The [full example collection](https://github.com/jolars/metewand/tree/main/examples)
also includes dataset-free problems, raw protocol workers, real-package
comparisons, external data providers, and release-overhead measurements.
