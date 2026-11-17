# Share and restore results

You can [export measurements](running.md#export-for-analysis) to analyze them in
a table or JSON file. To share the run together with its result files and
attempt history, create an archive.

## Share an archive {#share-stored-results}

Wait until every running attempt has finished. The archive can still include
failed attempts and work that has not started.

Create an archive of the run from the walkthrough, then check it:

```sh
metewand archive create --output-dir comparison-run --destination results.tar.gz
metewand archive verify --archive results.tar.gz
```

The archive contains the saved results, files, metrics, and failure history.
Verification checks that its records are consistent and intact. It does not run
the benchmark or repeat the scientific evaluation.

Someone with the archive can import and inspect it without your source checkout
or the original run directory:

```sh
metewand archive import --archive results.tar.gz --destination imported
metewand status --output-dir imported --verbose
```

The imported collection is read-only. You can inspect it and export tables from
it. Choose new paths for the archive and imported collection, with parents that
already exist. Keep the archive outside the run it contains.

## Continue from an archive

If you lose the original run directory and want to continue executing the
benchmark, restore the archive to a new directory on local storage:

```sh
metewand archive restore --archive results.tar.gz --destination restored
metewand run --manifest metewand.yaml --output-dir restored
```

You need the original benchmark sources, inputs, and compatible worker
environments to continue. Metewand reuses matching completed measurements and
keeps earlier failures. Request `--retry` if you want to retry those failures,
just as you would with the [original run](running.md#resume-and-diagnose).
