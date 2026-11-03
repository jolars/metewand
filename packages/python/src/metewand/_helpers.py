"""Optional conveniences over the stable lifecycle and checkpoint callbacks."""

from contextlib import contextmanager


@contextmanager
def stream_callback(stream, snapshot, *, work_field=None, start=0):
    """Yield a Boolean callback and finish a naturally terminated trajectory.

    ``snapshot`` is called only when the stream captures an observation. It may
    return JSON data or a Result with files in ``stream.output_dir``. Exceptions
    leave previously published checkpoints intact without fabricating a finish.
    """
    if type(start) is not int or not 0 <= start <= 2**53 - 1:
        raise ValueError("start must be a nonnegative safe integer")
    if work_field is not None and (not isinstance(work_field, str) or not work_field):
        raise ValueError("work_field must be a nonempty string")
    index = start
    stopped = False

    def progress():
        if index > 2**53 - 1:
            raise ValueError("checkpoint index exceeds the safe integer range")
        return {
            "check_index": index,
            "work": {} if work_field is None else {work_field: index},
        }

    def callback():
        nonlocal index, stopped
        if stopped:
            raise RuntimeError("checkpoint callback called after stop")
        stopped = stream.checkpoint(progress(), snapshot).stop
        index += 1
        return not stopped

    yield callback
    if not stopped:
        stream.finish(progress(), snapshot)
