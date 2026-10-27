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
  list(answer = sum(unlist(state$values)) + state$offset,
       input_count = length(state$values))
}

evaluate <- function(dataset_instance, problem_contract, problem_parameters,
                     problem_semantics, result_dir, metrics_path) {
  values <- read_json(file.path(dataset_instance, "data.json"))$values
  answer <- read_json(file.path(result_dir, "result.json"))$data$answer
  error <- abs(answer - (sum(unlist(values)) + problem_parameters$offset))
  verdict <- if (problem_semantics$validity$exact_sum && error != 0) "invalid" else
    if (error <= problem_semantics$one_shot_completion$absolute_error) "reached" else "not_reached"
  evaluation(list(absolute_error = error), verdict)
}

role <- commandArgs(trailingOnly = TRUE)[[1L]]
cat(role, "stdout\n")
cat(role, "stderr\n", file = stderr())
switch(role,
  implementation = serve_implementation(
    prepare, execute,
    result_data = function(output) list(answer = output$answer),
    reset = function(state) NULL,
    statistics = function(output) list(input_count = output$input_count)),
  dataset_materializer = serve_dataset_materializer(materialize),
  problem_evaluator = serve_evaluator(evaluate, semantics = TRUE),
  stop("Unknown worker role: ", role))
