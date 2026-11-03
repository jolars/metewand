from metewand import read_json


def generate(context):
    return {"values": list(range(context.parameters["size"]))}


def solve(data, context):
    grid = read_json(context.problem_dir / "data.json")["alpha"]
    return {"total": sum(data["values"]) * sum(grid)}


def solve_loop(data, context):
    grid = read_json(context.problem_dir / "data.json")["alpha"]
    return {"total": sum(value * alpha for value in data["values"] for alpha in grid)}


def evaluate(data, result, context):
    grid = read_json(context.problem_dir / "data.json")["alpha"]
    expected = sum(data["values"]) * sum(grid)
    return {"error": abs(result["total"] - expected)}
