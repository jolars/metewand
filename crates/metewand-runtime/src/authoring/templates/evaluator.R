library(metewand)

evaluate <- function(dataset_instance, problem_contract, problem_parameters,
                     problem_semantics, result_dir, metrics_path) {
  result <- read_json(file.path(result_dir, "result.json"))$data
  # TODO: Compute independent metrics from the dataset, parameters, and result.
  # Read validity and completion targets from problem_semantics. Return
  # evaluation(metrics, verdict), choosing reached, not_reached, or invalid.
  stop(worker_error("operation_failed", "TODO: Implement independent metrics and a verdict."))
}

serve_evaluator(evaluate, semantics = TRUE)
