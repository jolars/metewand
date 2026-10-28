entrypoint <- sub("^--file=", "", grep("^--file=", commandArgs(), value = TRUE))
library_dir <- file.path(dirname(entrypoint), "library")
if (dir.exists(library_dir)) .libPaths(c(library_dir, .libPaths()))
library(metewand)

materialize <- function(context) {
  list(values = context$parameters$values, seed = context$seed)
}

prepare <- function(context) {
  list(values = read_json(file.path(context$dataset_dir, "data.json"))$values,
       offset = context$problem_parameters$offset)
}

execute <- function(state, context) {
  list(answer = sum(unlist(state$values)) + state$offset,
       input_count = length(state$values))
}

evaluate <- function(context) {
  values <- read_json(file.path(context$dataset_dir, "data.json"))$values
  answer <- read_json(file.path(context$result_dir, "result.json"))$data$answer
  error <- abs(answer - (sum(unlist(values)) + context$problem_parameters$offset))
  verdict <- if (context$semantics$validity$exact_sum && error != 0) "invalid" else
    if (error <= context$semantics$one_shot_completion$absolute_error) "reached" else "not_reached"
  evaluation(list(absolute_error = error), verdict)
}

role <- commandArgs(trailingOnly = TRUE)[[1L]]
cat(role, "stdout\n")
cat(role, "stderr\n", file = stderr())
switch(role,
  implementation = serve_implementation(
    prepare = prepare, execute = execute,
    result = function(output, context) result(list(answer = output$answer)),
    reset = function(state, context) NULL,
    statistics = function(output, context) list(input_count = output$input_count)),
  dataset_materializer = serve_dataset_materializer(materialize),
  problem_evaluator = serve_evaluator(evaluate),
  stop("Unknown worker role: ", role))
