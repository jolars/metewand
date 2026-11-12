"""Integer sums with deliberate delays for exercising the terminal UI."""

from functools import reduce
import operator
import random
import time


def generate(context):
    # Keep preparation visible before the measurement progress bars appear.
    time.sleep(0.5)
    rng = random.Random(context.seed)
    return {"values": [rng.randint(-1000, 1000)
                       for _ in range(context.parameters["size"])]}


def builtin_sum(data, context):
    time.sleep(context.parameters["delay_seconds"])
    return {"total": sum(data["values"])}


def manual_loop(data, context):
    time.sleep(context.parameters["delay_seconds"])
    total = 0
    for value in data["values"]:
        total += value
    return {"total": total}


def reduce_sum(data, context):
    time.sleep(context.parameters["delay_seconds"])
    return {"total": reduce(operator.add, data["values"], 0)}


def evaluate(data, result, context):
    return {"absolute_error": abs(result["total"] - sum(data["values"]))}
