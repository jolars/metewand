# Replace these functions with your data, methods, and independent metrics.

generate <- function(context) {
  set.seed(context$seed %% .Machine$integer.max)
  list(values = as.list(sample.int(10L, context$parameters$size, replace = TRUE) - 1L))
}

builtin_sum <- function(data, context) {
  list(total = sum(unlist(data$values)))
}

loop_sum <- function(data, context) {
  total <- 0
  for (value in data$values) total <- total + value
  list(total = total)
}

evaluate <- function(data, result, context) {
  list(absolute_error = abs(result$total - sum(unlist(data$values))))
}
