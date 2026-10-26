library(metewand)

materialize <- function(source_dir, dataset_parameters, seed, output_dir) {
  # TODO: Construct the scientific dataset and write its files into output_dir.
  # Use write_json, file_entry, and write_dataset_manifest from metewand;
  # return the relative manifest path after closing every file.
  stop(worker_error("operation_failed", "TODO: Implement dataset construction and conversion."))
}

serve_dataset_materializer(materialize)
