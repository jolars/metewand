# Metewand Python SDK

This package provides small helpers for Python implementation, dataset
materializer, and independent evaluator workers. It requires Python 3.11 or
later and has no runtime dependencies.

Install the local package into the Python environment that will run your worker:

```sh
python3 -m pip install ./sdk/python
```

The package implements Metewand's version-1 one-shot protocol. It serializes
messages, dispatches callbacks, reports structured errors, and measures the
implementation callback. Metewand owns environment provisioning, source
acquisition, seeds, scheduling, worker lifecycle, artifact validation, and
durable publication. The SDK does not set random generators or infer scientific
semantics.

For simple adapters, subclass `OneShotImplementation` and return scientific data
from `execute`. Evaluators return `Evaluation(metrics, verdict)`. The SDK writes
standard envelopes; custom result and metrics writers remain supported.

See [the API guide](../../docs/reference/python-sdk.md) in the repository and the runnable
[Python SDK example](../../examples/python-sdk). Run the SDK tests from the
repository root with:

```sh
PYTHONPATH=sdk/python/src python3 -m unittest discover -s sdk/python/tests -v
```

The normal Rust workspace tests also run this suite and exercise all three
Python roles through the Rust transport and output validator.
