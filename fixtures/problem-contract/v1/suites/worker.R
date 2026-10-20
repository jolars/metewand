library(metewand)
args <- commandArgs(trailingOnly = TRUE)
role <- args[[1L]]
family <- args[[2L]]
mode <- args[[3L]]

prepare <- function(dataset, contract, problem, implementation, seed) {
  list(values = read_json(file.path(dataset, "data.json"))$values, parameters = problem)
}
execute <- function(state, budget, control) {
  values <- unlist(state$values)
  if (family == "sum") {
    result <- list(answer = sum(values) + state$parameters$offset, count = length(values))
    numeric <- "answer"
    semantic <- "count"
  } else {
    index <- which.min(values)
    result <- list(index = index - 1, value = values[[index]])
    numeric <- "value"
    semantic <- "index"
  }
  if (mode == "missing_field") result[[semantic]] <- NULL
  else if (mode == "wrong_type") result[[numeric]] <- "wrong"
  else if (mode == "numerically_wrong") result[[numeric]] <- result[[numeric]] + 1
  else if (mode == "semantically_invalid") result[[semantic]] <- result[[semantic]] + 1
  result
}
result <- function(state, directory) write_result(directory, state, schema = "schemas/result.json")

evaluate <- function(dataset, contract, parameters, result, metrics) {
  values <- read_json(file.path(dataset, "data.json"))$values
  candidate <- read_json(file.path(result, "result.json"))$data
  if (family == "sum") {
    expected <- parameters$offset
    for (value in values) expected <- expected + value
    valid <- candidate$count == length(values)
    error <- abs(candidate$answer - expected)
  } else {
    expected <- sort(unlist(values))[[1L]]
    first <- which(unlist(values) == expected)[[1L]] - 1
    valid <- candidate$index == first
    error <- abs(candidate$value - expected)
  }
  write_metrics(metrics, list(absolute_error = error, semantically_valid = valid), schema = "schemas/metrics.json")
  if (!valid) "invalid" else if (error == 0) "reached" else "not_reached"
}

if (role == "implementation") {
  serve_implementation(prepare, execute, result)
} else {
  serve_evaluator(evaluate)
}
