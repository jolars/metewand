# Local examples

These are ordinary benchmark repositories with author-owned contracts and
independent evaluators. Metewand does not give them special template behavior.
Run commands from the Metewand repository root and choose a fresh output path
for every run. Local environments record observed controls and provenance;
they do not provide container isolation or a locked publication environment.

| Form | Example | Requirements | Expected observations |
| --- | --- | --- | --- |
| Raw command | [Fixed dataset](fixed-dataset) | Python 3, standard library only | One answer of 8 |
| Python-only | [Python SDK](python-sdk) | Python 3.11+, staged in-tree SDK | Two answers of 8 |
| R-only | [R SDK](r-sdk) | R, digest, C++17 compiler, installed in-tree SDK | Two answers of 8 |
| Dataset-free | [Integer sum](dataset-free) | Python 3, standard library only | Answers 6 and 55 |
| Parameter matrix | [Grids and coupled cases](parameter-matrix) | Python 3, standard library only | Ten answers across five configurations and two representations |

Each linked README provides complete setup and run commands. Python-only and
R-only refer to the benchmark workers; the Metewand runner is the Rust CLI.
Their simple arithmetic contracts make evaluation transparent without choosing
a scientific domain for new benchmarks. The SDKs are copied or installed
explicitly into temporary benchmark directories before running.

Further examples demonstrate [generated datasets](generated-dataset) and an
optional [Datamonger source adapter](datamonger). See the
[authoring guide](../docs/authoring-cli.md) to create your own benchmark and the
[run guide](../docs/run-cli.md) for output and execution policies.
