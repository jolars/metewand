library(metewand)

prepare <- function(dataset_instance, problem_contract, problem_parameters,
                    implementation_parameters, seed) {
  # TODO: Load the dataset, translate parameters, and return solver state.
  stop(worker_error("operation_failed", "TODO: Implement scientific preparation."))
}

execute <- function(state, budget, observation_control) {
  # TODO: Run the method and return scientific result data as a named list.
  stop(worker_error("operation_failed", "TODO: Implement the method and result conversion."))
}

serve_implementation(prepare, execute)
