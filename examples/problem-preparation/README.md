# Shared problem preparation

This small example models a shared solution-path grid without scientific package
dependencies. Dataset size belongs to the dataset. `path_length`, `q`, and
`fit_intercept` belong to the problem. The preparer derives one grid for each
dataset and problem configuration, including a path that stops before the
requested length. Both implementations and the evaluator read that exact grid
from `context.problem_dir`.

Install the Python package in the selected environment, then run from this
directory:

```sh
python3 -m pip install ../../packages/python
metewand check --workers
metewand plan
metewand run --output-dir first-run --cache-dir cache
metewand run --output-dir second-run --cache-dir cache
```

Each run produces eight observations: two path configurations, two methods, and
two measurements. The second run reuses both dataset and problem preparation.
Changing `q` or `fit_intercept` rebuilds the shared grids while reusing the raw
dataset. Shared preparation remains outside every implementation's timing scope.
