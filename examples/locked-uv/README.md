# Locked uv workers

With `metewand`, uv, and Python 3.11 or newer installed:

```sh
python3 examples/locked-uv/run.py /tmp/metewand-locked-uv
```

The script copies this benchmark to a fresh directory, prepares its environment,
verifies it, runs two implementations, resumes the completed run, and shows its
status. Results go to the sibling `metewand-locked-uv-results` directory. Each
implementation returns 8, which a separate evaluator checks. The dataset
materializer and the evaluator's reference case use the same prepared interpreter.

The manifest explicitly selects the `benchmark` dependency group and the
`checked` extra. Both depend on the local `arithmetic` package. Its small build
backend uses Python's standard library so preparation needs no package downloads.
The implementation imports the installed package; the evaluator computes its
expected answer independently. The native `uv.lock` belongs to the author.

Prepared environments live in Metewand's persistent user data directory. Set
`METEWAND_DATA_DIR` to an absolute directory outside the benchmark sources to
choose another location. The project `.venv` is neither selected nor modified.

After preparation, these commands work offline without uv or Nix installed:

```sh
metewand lock --check
metewand run --locked --output-dir ../another-result
```

Metewand checks source content, the interpreter and standard library, installed
files, and exact worker launches before creating run output. Source-only local
package changes require `metewand lock` again. Changed installed files fail
verification; explicit preparation builds a fresh generation while retaining the
previous one. Missing prepared environments also require explicit preparation.

The evidence identifies the prepared Python runtime and installed packages.
External native libraries and build tools remain host dependencies, and workers
retain the local executor's network and filesystem controls. A uv lock does not
establish an isolated archival environment.

## Qualify clustlasso-cd

The optional staging script reuses the existing analytic PG and hybrid comparison
and its independent evaluator from `examples/adopters`. It copies the current
adopter package sources, native lock, build inputs, and Python binding helpers
without editing the adopter checkout. The uv environment selects the adopter's
`benchmark` dependency group and builds its native extension.

```sh
python3 examples/locked-uv/stage_clustlasso.py \
  --clustlasso /path/to/clustlasso-cd --destination /tmp/clustlasso-uv
metewand lock --manifest /tmp/clustlasso-uv/metewand.yaml
metewand lock --check --manifest /tmp/clustlasso-uv/metewand.yaml
metewand run --locked --manifest /tmp/clustlasso-uv/metewand.yaml \
  --output-dir /tmp/clustlasso-uv-results
```

Install the Python version pinned by the adopter before preparation. On hosts
that need an explicit native library search path, pass `--library-path PATHS`
to staging. The copied worker launcher sets that declared path and numerical
thread counts before importing NumPy or the compiled solver. These remain
explicit host dependencies. The retained `clustlasso-qualification.json` records
the local qualification's package versions, accepted comparisons, and checked
metrics.
