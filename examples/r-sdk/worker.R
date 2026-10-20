entrypoint <- sub("^--file=", "", grep("^--file=", commandArgs(), value = TRUE))
library_dir <- file.path(dirname(entrypoint), "library")
if (dir.exists(library_dir)) .libPaths(c(library_dir, .libPaths()))
library(metewand)

materialize <- function(source_dir, dataset_parameters, seed, output_dir) {
  write_json(file.path(output_dir, "data.json"),
             list(values = dataset_parameters$values, seed = seed))
  write_dataset_manifest(output_dir,
    list(file_entry(output_dir, "data.json", "application/json")))
}

prepare <- function(dataset_instance, problem_contract, problem_parameters,
                    implementation_parameters, seed) {
  list(values = read_json(file.path(dataset_instance, "data.json"))$values,
       offset = problem_parameters$offset)
}

execute <- function(state, budget, observation_control) {
  state$answer <- sum(unlist(state$values)) + state$offset
  state
}

result <- function(state, directory) {
  write_result(directory, list(answer = state$answer), schema = "schemas/result.json")
}

evaluate <- function(dataset_instance, problem_contract, problem_parameters,
                     result_dir, metrics_path) {
  values <- read_json(file.path(dataset_instance, "data.json"))$values
  answer <- read_json(file.path(result_dir, "result.json"))$data$answer
  error <- abs(answer - (sum(unlist(values)) + problem_parameters$offset))
  write_metrics(metrics_path, list(absolute_error = error), schema = "schemas/metrics.json")
  if (error == 0) "reached" else "invalid"
}

role <- commandArgs(trailingOnly = TRUE)[[1L]]
cat(role, "stdout\n")
cat(role, "stderr\n", file = stderr())
switch(role,
  implementation = serve_implementation(
    prepare, execute, result,
    reset = function(state) NULL,
    statistics = function(state) list(input_count = length(state$values))),
  dataset_materializer = serve_dataset_materializer(materialize),
  problem_evaluator = serve_evaluator(evaluate),
  stop("Unknown worker role: ", role))
