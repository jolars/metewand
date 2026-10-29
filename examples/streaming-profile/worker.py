"""One trajectory, paused while the problem independently evaluates each checkpoint."""

from metewand import serve_implementation


def prepare(context):
    return context.parameters["maximum_updates"]


def execute_stream(maximum_updates, context):
    stream = context.stream
    x = 1.0
    for index in range(maximum_updates + 1):
        progress = {"check_index": index, "work": {"updates": index}}
        if stream.checkpoint(progress, lambda: {"x": x}).stop:
            return
        if index == maximum_updates:
            stream.finish(progress, lambda: {"x": x})
            return
        x *= 0.5


if __name__ == "__main__":
    serve_implementation(prepare=prepare, execute_stream=execute_stream,
                         reset=lambda state, context: None)
