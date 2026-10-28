"""One unchanged trajectory, with direct recording or a local observer."""

from metewand import serve_implementation


class Halving:
    def prepare(self, context):
        implementation_parameters = context.parameters
        self.maximum_updates = implementation_parameters["maximum_updates"]
        self.local_bias = implementation_parameters["local_bias"]
        return self

    def execute_buffered(self, state, context):
        trace = context.observer
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

    def reset(self, state, context):
        pass


if __name__ == "__main__":
    worker = Halving()
    serve_implementation(prepare=worker.prepare, execute_buffered=worker.execute_buffered, reset=worker.reset)
