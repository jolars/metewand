"""One unchanged trajectory, with direct recording or a local observer."""

from metewand import BufferedImplementation, serve_implementation


class Halving(BufferedImplementation):
    def prepare(self, dataset_instance, problem_contract, problem_parameters,
                implementation_parameters, seed):
        self.maximum_updates = implementation_parameters["maximum_updates"]
        self.local_bias = implementation_parameters["local_bias"]

    def execute_buffered(self, budget, trace):
        x = 1.0
        index = 0
        while True:
            progress = {"check_index": index, "work": {"updates": index}}
            measure = lambda: {"gap": max(0.0, x * x + self.local_bias)}
            snapshot = lambda: {"x": x}
            if trace.profile["instrumentation"]["recording"] == "direct":
                decision = trace.record(progress, measure(), snapshot)
            else:
                decision = trace.observe(progress, measure, snapshot)
            if decision.stop or index >= self.maximum_updates:
                break
            x *= 0.5
            index += 1
        trace.finish(progress, measure, snapshot)

    def reset(self):
        pass


if __name__ == "__main__":
    serve_implementation(Halving())
