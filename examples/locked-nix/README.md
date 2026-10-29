# Locked Nix workers

With Nix and `metewand` on `PATH`, run from the repository root:

```sh
python3 examples/locked-nix/run.py /tmp/metewand-locked
python3 examples/locked-nix/run.py /tmp/metewand-locked-app --app
```

Choose new destinations. The script copies the raw fixed-dataset comparison,
selects the current Nix system, prepares the pinned Python package or app,
checks `metewand.lock`, and executes a locked run. Results go to a sibling
directory. The workers use Python's standard library and do not require the
Metewand Python package.

`flake.lock` pins the upstream Nixpkgs source. `metewand lock` leaves that file
unchanged and writes the prepared environment evidence to `metewand.lock`.
Repeated preparation produces the same bytes until an input changes. Commit
the generated lockfile with the benchmark. In Git repositories, stage new
source files before locking; edits to tracked files are captured directly.

`metewand lock --check` and `metewand run --locked` inspect prepared local paths
without fetching or building software. If garbage collection removes an output,
run `metewand lock` again. These environments provide exact software evidence
while retaining the trusted local executor's existing control limitations.
