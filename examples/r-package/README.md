# R package example

This benchmark generates `[1, 2, 3]`, sums the values and adds two, then evaluates
the answer independently. It exercises all three package roles, two measured slots,
process reuse, and durable publication.

With R and `jsonlite` in the selected environment, run from the
repository root:

```sh
demo_dir=$(mktemp -d)
cp -R examples/r-package/. "$demo_dir/"
mkdir "$demo_dir/library"
R CMD INSTALL --library="$demo_dir/library" packages/r
cargo run --locked -p metewand-cli -- run \
  --manifest "$demo_dir/metewand.yaml" \
  --output-dir "$demo_dir/run" --output jsonl
```

On native Windows, put `R.exe` and
`Rscript.exe` on `PATH`. In PowerShell:

```powershell
$demo = Join-Path $env:TEMP ([guid]::NewGuid().ToString())
Copy-Item examples/r-package $demo -Recurse
New-Item "$demo/library" -ItemType Directory
R CMD INSTALL "--library=$demo/library" packages/r
cargo run --locked -p metewand-cli -- run --manifest "$demo/metewand.yaml" --output-dir "$demo/run" --output jsonl
```

Use local NTFS for the temporary directory and install `jsonlite` into the R
interpreter's site library as described in [getting started](../../docs/guide/getting-started.md#start-with-r).

The explicit source bundle includes `worker.R` and the installed `metewand`
package under `library/`. The worker loads that staged package using an explicit
library path beside its entrypoint. Install `jsonlite` in the selected
environment's site library; the local launcher assigns a private `R_LIBS_USER`
and does not inherit an ambient library path. The repository's development
environment supplies R and `jsonlite` together.

Both observations contain `{"answer":8}` and evaluator metrics
`{"absolute_error":0}`. Dataset construction happens once. `prepare` loads the
private dataset view, `execute` computes the answer, and `result(output, context)` selects
the scientific output. The CLI writes the completed envelope. The evaluator
returns `evaluation(metrics, verdict)` so the CLI also writes metrics. The
`prepare_and_execute` timing scope includes preparation, solving, conversion,
and result writing. R secondary solve timing is unavailable. The runtime
resets the implementation between slots. The evaluator independently computes
the expected answer from its dataset and problem parameters.

See the R package help for [workers](../../packages/r/man/workers.Rd),
[JSON utilities](../../packages/r/man/json.Rd), and
[artifacts](../../packages/r/man/artifacts.Rd).
