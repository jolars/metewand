"""Shared eligibility for periodic and explicit observation schedules."""

from bisect import bisect_right

from ._canonical import MAX_SAFE_INTEGER


class CheckSchedule:
    def __init__(self, specification):
        from ._worker import WorkerError

        def valid_integer(value, minimum):
            return type(value) is int and minimum <= value <= MAX_SAFE_INTEGER
        self.indices = specification.get("indices")
        self.every = specification.get("every")
        self.start = specification.get("start", 0)
        if self.indices is not None:
            valid = (
                set(specification) == {"indices"}
                and type(self.indices) is list
                and bool(self.indices)
                and all(valid_integer(n, 0) for n in self.indices)
                and all(a < b for a, b in zip(self.indices, self.indices[1:]))
            )
        else:
            valid = (
                set(specification) <= {"every", "start"}
                and valid_integer(self.every, 1)
                and valid_integer(self.start, 0)
            )
        if not valid:
            raise WorkerError("operation_failed", "Invalid checkpoint schedule.")

    def includes(self, index):
        if self.indices is not None:
            position = bisect_right(self.indices, index)
            return position > 0 and self.indices[position - 1] == index
        return index >= self.start and (index - self.start) % self.every == 0

    def next_after(self, previous=None):
        if self.indices is not None:
            position = 0 if previous is None else bisect_right(self.indices, previous)
            return self.indices[position] if position < len(self.indices) else None
        if previous is None or previous < self.start:
            return self.start
        value = self.start + ((previous - self.start) // self.every + 1) * self.every
        return value if value <= MAX_SAFE_INTEGER else None
