"""Ordinary scientific functions backed by the existing worker lifecycle."""

from dataclasses import dataclass
import importlib.util
from pathlib import Path
import sys

from ._artifacts import file_entry, read_json, write_dataset_manifest, write_json
from ._worker import Evaluation, OneShotImplementation, _serve_fds


@dataclass(frozen=True)
class FunctionContext:
    """Role parameters, problem parameters, seed, and declared scientific rules."""

    parameters: dict
    problem_parameters: dict
    seed: int | None
    semantics: dict


def materialize_function(function, source_dir, parameters, seed, output_dir):
    data = function(FunctionContext(parameters, {}, seed, {}))
    if type(data) is not dict:
        raise TypeError("A dataset function must return a JSON-compatible object.")
    write_json(output_dir / "data.json", data)
    return write_dataset_manifest(output_dir, [file_entry(output_dir, "data.json", "application/json")])


def read_function_data(dataset):
    path = dataset / "data.json"
    if not path.exists() and not any(dataset.iterdir()):
        return {}
    data = read_json(path)
    if type(data) is not dict:
        raise TypeError("A function dataset must contain a JSON object in data.json.")
    return data


class FunctionImplementation(OneShotImplementation):
    def __init__(self, function):
        self.function = function

    def prepare(self, dataset, contract, problem_parameters, parameters, seed):
        self.data = read_function_data(dataset)
        self.context = FunctionContext(parameters, problem_parameters, seed, {})

    def execute(self, budget, observation_control):
        return self.function(self.data, self.context)

    def reset(self):
        self.data = None
        self.context = None


def evaluate_function(function, data, result, context):
    outcome = function(data, result, context)
    completion = context.semantics.get("one_shot_completion", {})
    if isinstance(outcome, Evaluation):
        if not completion and outcome.verdict in {"reached", "not_reached"}:
            raise ValueError("A completion verdict requires declared completion rules; use not_required or invalid.")
        if completion and outcome.verdict == "not_required":
            raise ValueError("Declared completion rules require an explicit completion verdict.")
        return outcome
    if context.semantics.get("validity") or completion:
        raise ValueError("Scientific validity or completion rules require an explicit Evaluation verdict.")
    return Evaluation(outcome, "not_required")


def serve_function(role, file, name):
    """Load one explicitly selected function inside an authorized worker process."""
    file = Path(file)
    function = None

    def initialize():
        nonlocal function
        # Loading during hello excludes imports from default timing and reports errors.
        sys.path.insert(0, str(file.parent))
        spec = importlib.util.spec_from_file_location("_metewand_benchmark", file)
        if spec is None or spec.loader is None:
            raise ValueError(f"Cannot load function source {file}.")
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        function = getattr(module, name, None)
        if not callable(function):
            raise ValueError(f"{file}:{name} does not identify a callable function.")

    def call(*args):
        return function(*args)

    if role == "dataset_materializer":
        _serve_fds(lambda *args: materialize_function(call, *args), role, None, None, initialize=initialize)
    elif role == "implementation":
        _serve_fds(FunctionImplementation(call), role, None, None, initialize=initialize)
    elif role == "problem_evaluator":
        def evaluate(dataset, contract, parameters, semantics, result_dir, metrics_path):
            data = read_function_data(dataset)
            result = read_json(result_dir / "result.json")["data"]
            return evaluate_function(call, data, result, FunctionContext(parameters, parameters, None, semantics))
        _serve_fds(evaluate, role, None, None, semantics=True, initialize=initialize)
    else:
        raise ValueError(f"Unknown function worker role: {role}.")
