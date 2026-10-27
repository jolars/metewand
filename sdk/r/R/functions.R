.function_context <- function(parameters, problem_parameters = json_object(), seed = NULL,
                              semantics = json_object()) {
  list(parameters = parameters, problem_parameters = problem_parameters,
       seed = seed, semantics = semantics)
}

.read_function_data <- function(dataset) {
  path <- file.path(dataset, "data.json")
  if (!file.exists(path) && length(list.files(dataset, all.files = TRUE, no.. = TRUE)) == 0L) {
    return(json_object())
  }
  data <- read_json(path)
  .object(data, "Function dataset data.json")
  data
}

.evaluate_function <- function(callback, data, result, context) {
  outcome <- callback(data, result, context)
  completion <- length(context$semantics$one_shot_completion) > 0L
  if (inherits(outcome, "metewand_evaluation")) {
    if (!completion && outcome$verdict %in% c("reached", "not_reached")) {
      stop("A completion verdict requires declared completion rules; use not_required or invalid.", call. = FALSE)
    }
    if (completion && outcome$verdict == "not_required") {
      stop("Declared completion rules require an explicit completion verdict.", call. = FALSE)
    }
    return(outcome)
  }
  if (length(context$semantics$validity) > 0L || completion) {
    stop("Scientific validity or completion rules require an explicit evaluation verdict.", call. = FALSE)
  }
  evaluation(outcome, "not_required")
}

serve_function <- function(role = NULL, file = NULL, name = NULL) {
  if (is.null(role)) {
    arguments <- commandArgs(trailingOnly = TRUE)
    if (length(arguments) != 3L) stop("Expected worker role, source file, and function name.", call. = FALSE)
    role <- arguments[[1L]]
    file <- arguments[[2L]]
    name <- arguments[[3L]]
  }
  callback <- NULL
  initialize <- function() {
    # Loading during hello excludes imports from default timing and reports errors.
    environment <- new.env(parent = globalenv())
    sys.source(file, envir = environment, chdir = TRUE)
    if (!exists(name, envir = environment, inherits = FALSE) ||
        !is.function(environment[[name]])) {
      stop(paste0(file, ":", name, " does not identify a callable function."), call. = FALSE)
    }
    callback <<- environment[[name]]
  }
  if (role == "dataset_materializer") {
    worker <- function(source, parameters, seed, output) {
      data <- callback(.function_context(parameters, seed = seed))
      .object(data, "Dataset data")
      write_json(file.path(output, "data.json"), data)
      write_dataset_manifest(output, list(file_entry(output, "data.json", "application/json")))
    }
    .serve_fds(worker, role, NULL, NULL, initialize = initialize)
  } else if (role == "implementation") {
    worker <- list(
      prepare = function(dataset, contract, problem, parameters, seed) {
        list(data = .read_function_data(dataset), context = .function_context(parameters, problem, seed))
      },
      execute = function(state, budget, observation_control) callback(state$data, state$context),
      reset = function(state) NULL)
    .serve_fds(worker, role, NULL, NULL, initialize = initialize)
  } else if (role == "problem_evaluator") {
    worker <- function(dataset, contract, parameters, semantics, result_dir, metrics_path) {
      result <- read_json(file.path(result_dir, "result.json"))$data
      .evaluate_function(callback, .read_function_data(dataset), result,
                         .function_context(parameters, parameters, semantics = semantics))
    }
    .serve_fds(worker, role, NULL, NULL, semantics = TRUE, initialize = initialize)
  } else {
    stop(paste0("Unknown function worker role: ", role), call. = FALSE)
  }
}
