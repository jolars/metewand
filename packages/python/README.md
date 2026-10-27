# Metewand Python package

Define Metewand benchmarks with ordinary Python functions for data generation,
methods, and independent evaluation. The package handles worker communication
and standard result files. It requires Python 3.11 or later and has no runtime
dependencies.

Install the local package into the Python environment that will run your worker:

```sh
python3 -m pip install ./packages/python
```

The package implements Metewand's version-1 one-shot protocol. It serializes
messages, dispatches callbacks, reports structured errors, and measures the
implementation callback. Metewand owns environment provisioning, source
acquisition, seeds, scheduling, worker lifecycle, artifact validation, and
durable publication. The package does not set random generators or infer scientific
semantics.

For a simple benchmark, use `metewand init` and ordinary functions selected in
YAML with `function: benchmark.py:solve`. The package supplies the context, JSON data,
and artifact envelopes. Evaluators can return plain metrics when no scientific
rules are declared; completion is then `not_required`. See the
[least-squares example](../../examples/least-squares) for incremental validation.

For lifecycle adapters, subclass `OneShotImplementation` and return scientific data
from `execute`. Evaluators return `Evaluation(metrics, verdict)`. The package writes
standard envelopes; custom result and metrics writers remain supported.

See the [authoring guide](../../docs/guide/authoring/index.md),
[callback interfaces](src/metewand/_worker.py), and the runnable
[Python package example](../../examples/python-package). Run the package tests from the
repository root with:

```sh
PYTHONPATH=packages/python/src python3 -m unittest discover -s packages/python/tests -v
```

The normal Rust workspace tests also run this suite and exercise all three
Python roles through the Rust transport and output validator.

### Configuration-dependent support

Declare `capabilities: [one_shot, applicability]` and override
`check_applicability` to exclude configurations your implementation cannot
support intrinsically:

```python
def check_applicability(self, dataset_instance, dataset_manifest,
                        problem_contract, problem_parameters,
                        implementation_parameters):
    supported = not problem_parameters["intercept"]
    return {
        "outcome": "applicable" if supported else "not_applicable",
        "code": "supported" if supported else "unsupported_intercept",
        "reason": "Supported configuration." if supported else "Requires a zero intercept.",
    }
```

Both dataset arguments are `Path` objects. The callback runs in a separate
worker before measured preparation, may inspect the immutable dataset and its
manifest, and must return nonempty `code` and `reason` strings. Optional
`details` must be a dictionary. Exclusions produce no attempts; callback
exceptions fail resolution. `plan` records `unchecked` until the dataset and
worker are resolved. `explain` and `export` retain checked inputs and reasons.
