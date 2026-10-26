entrypoint <- sub("^--file=", "", grep("^--file=", commandArgs(), value = TRUE))
library_dir <- file.path(dirname(entrypoint), "library")
if (dir.exists(library_dir)) .libPaths(c(library_dir, .libPaths()))
library(metewand)

prepare <- function(dataset, contract, problem, parameters, seed) {
  list(seed = seed)
}
execute <- function(state, budget, observation_control) {
  answer <- stats::uniroot(function(x) x * x - 2, c(1, 2),
                           tol = observation_control$tolerance)
  list(x = answer$root, seed = state$seed)
}
serve_implementation(
  prepare, execute,
  result_data = function(output) list(x = output$x),
  statistics = function(output) list(pid = Sys.getpid(), seed = output$seed),
  reset = function(state) NULL,
  fresh_sequence = TRUE)
