from metewand import OneShotImplementation, WorkerError, serve_implementation


class Solver(OneShotImplementation):
    def prepare(self, dataset_instance, problem_contract, problem_parameters,
                implementation_parameters, seed):
        # TODO: Load the dataset, translate parameters, and initialize solver state.
        raise WorkerError("operation_failed", "TODO: Implement scientific preparation.")

    def execute(self, budget, observation_control):
        # TODO: Run the method and return scientific result data as a dict.
        raise WorkerError("operation_failed", "TODO: Implement the method and result conversion.")


if __name__ == "__main__":
    serve_implementation(Solver())
