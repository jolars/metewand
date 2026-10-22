# Examples

The repository includes small benchmarks with explicit problem contracts and
independent evaluators. Run the commands in each example's README from the
repository root.

  | Example                                                                                      | What it demonstrates                                                      |
  | -------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
  | [Fixed dataset](https://github.com/jolars/metewand/tree/main/examples/fixed-dataset)         | A complete run with raw Python workers and a fixed input dataset.         |
  | [Python SDK](https://github.com/jolars/metewand/tree/main/examples/python-sdk)               | Dataset, implementation, and evaluator callbacks, with process reuse.     |
  | [R SDK](https://github.com/jolars/metewand/tree/main/examples/r-sdk)                         | The same three worker roles using the R SDK.                              |
  | [Generated dataset](https://github.com/jolars/metewand/tree/main/examples/generated-dataset) | Materialization and implementation preparation with validated artifacts.  |
  | [Dataset-free](https://github.com/jolars/metewand/tree/main/examples/dataset-free)           | A problem using the built-in empty dataset.                               |
  | [Parameter matrix](https://github.com/jolars/metewand/tree/main/examples/parameter-matrix)   | Cartesian grids and coupled named cases in explicit parameter namespaces. |
  | [Datamonger](https://github.com/jolars/metewand/tree/main/examples/datamonger)               | A pinned dataset resolved through an external provider process.           |

Start with [getting started](getting-started.md) for the fixed-dataset example.
The [example index](https://github.com/jolars/metewand/tree/main/examples)
describes requirements and expected results for the full collection.
