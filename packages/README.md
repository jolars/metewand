# Metewand language packages

Use the Metewand package for your language to define data generators, methods,
and evaluators as ordinary functions. Declare the comparison in YAML, then use
the `metewand` CLI to check, run, and inspect it.

  | Package          | Install from this checkout                 |
  | ---------------- | ------------------------------------------ |
  | [Python](python) | `python3 -m pip install ./packages/python` |
  | [R](r)           | `R CMD INSTALL packages/r`                 |

Run these commands from the repository root in the environment selected for your
benchmark. Neither package is published to a registry yet. See each package's
README for requirements, and follow [getting
started](../docs/guide/getting-started.md) for a complete benchmark.

The packages handle worker communication and standard result files. Lifecycle
callbacks support separate preparation, custom artifacts, and profiles as a
benchmark grows. Workers run as separate processes, so a benchmark can combine
languages. The repository's native Rust examples use [internal worker
helpers](../src/runtime/worker_sdk.rs). Metewand currently publishes only the CLI,
with no Rust library API. Other native workers implement the process protocol
directly.
