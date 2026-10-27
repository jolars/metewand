"""Replace these functions with your data, methods, and independent metrics."""

import random


def generate(context):
    rng = random.Random(context.seed)
    return {"values": [rng.randrange(10) for _ in range(context.parameters["size"])]}


def builtin_sum(data, context):
    return {"total": sum(data["values"])}


def loop_sum(data, context):
    total = 0
    for value in data["values"]:
        total += value
    return {"total": total}


def evaluate(data, result, context):
    return {"absolute_error": abs(result["total"] - sum(data["values"]))}
