# Local Apptainer workers

Install Apptainer 1.3 or newer on Linux. Supply a native SIF image containing
Python 3 at `images/worker.sif`, relative to this directory. The file can remain
Git-ignored. For example, explicitly prepare an image yourself:

```sh
mkdir -p images
apptainer pull images/worker.sif docker://python:3.13-slim
metewand lock
metewand lock --check
metewand run --locked --output-dir ../apptainer-results
```

The pull is an authoring step. Metewand never pulls or builds images. The lock
binds the complete SIF bytes, so changing the image requires explicit relocking.
Supported images have a native amd64 or arm64 unencrypted SquashFS system
partition. Image paths must be normalized repository-relative paths without
symlinks. Keep image files outside worker source globs.

Materializers, implementations, and evaluators run in the image as the invoking
user, using authenticated sockets over host networking. CPU and memory accounting
are unavailable. Workers run trusted code; this backend does not provide archival
isolation guarantees.

For the opt-in integration test, supply an existing Python SIF:

```sh
METEWAND_TEST_SIF=/absolute/path/worker.sif compute run -- cargo test --test integration --locked cli_tests::locked_apptainer:: -- --ignored
```
