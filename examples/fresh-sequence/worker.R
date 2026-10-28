entrypoint <- sub("^--file=", "", grep("^--file=", commandArgs(), value = TRUE))
library_dir <- file.path(dirname(entrypoint), "library")
if (dir.exists(library_dir)) .libPaths(c(library_dir, .libPaths()))
library(metewand)

prepare <- function(context) {
  list(seed = context$seed)
}
execute <- function(state, context) {
  answer <- stats::uniroot(function(x) x * x - 2, c(1, 2),
                           tol = context$observation_control$tolerance)
  list(x = answer$root, seed = state$seed)
}
serve_implementation(
  prepare = prepare, execute = execute,
  result = function(output, context) list(x = output$x),
  statistics = function(output, context) list(pid = Sys.getpid(), seed = output$seed),
  reset = function(state, context) NULL)
