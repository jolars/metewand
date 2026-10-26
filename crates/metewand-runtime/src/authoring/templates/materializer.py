from metewand import WorkerError, serve_dataset_materializer


def materialize(source_dir, dataset_parameters, seed, output_dir):
    # TODO: Construct the scientific dataset and write its files into output_dir.
    # Use write_json, file_entry, and write_dataset_manifest from metewand;
    # return the relative manifest path after closing every file.
    raise WorkerError("operation_failed", "TODO: Implement dataset construction and conversion.")


if __name__ == "__main__":
    serve_dataset_materializer(materialize)
