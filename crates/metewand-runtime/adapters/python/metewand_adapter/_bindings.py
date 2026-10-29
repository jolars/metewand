"""Callback API version 1, implemented and released with the CLI."""

from dataclasses import asdict
import importlib.util
from pathlib import Path
import sys

import metewand as binding
from . import _worker
from ._artifacts import write_json, read_json

BINDING_API_VERSION = 1


def _invoke(callback, *args):
    try:
        return callback(*args)
    except binding.WorkerError as error:
        raise _worker.WorkerError(error.code, str(error), error.details) from error


def _draft(directory, kind, data, files=()):
    entries = []
    for file in files:
        if not isinstance(file, binding.Artifact):
            raise TypeError("Declare output files with Artifact.")
        entry = asdict(file)
        entry["path"] = (
            file.path.as_posix() if isinstance(file.path, Path) else str(file.path)
        )
        if entry["representation"] is None:
            entry.pop("representation")
        entries.append(entry)
    document = {"schema_version": 1, "kind": kind, "data": data, "files": entries}
    write_json(Path(directory) / ".metewand-output.json", document)
    return ".metewand-output.json"


def _context(request):
    parameters = request.get(
        "implementation_parameters",
        request.get("dataset_parameters", request.get("problem_parameters", {})),
    )
    fields = dict(
        parameters=parameters,
        problem_parameters=request.get("problem_parameters", {}),
        seed=request.get("implementation_seed", request.get("dataset_seed")),
        semantics=request.get("problem_semantics", {}),
        budget=None,
        observation_control=request.get("observation_control"),
    )
    fields.update(request)
    for key in (
        "dataset_dir",
        "source_dir",
        "result_dir",
        "output_dir",
        "metrics_path",
        "result_root",
        "dataset_manifest",
    ):
        if key in fields:
            fields[key] = Path(fields[key])
    fields["output_dir"] = fields.get(
        "output_dir", fields.get("result_dir", fields.get("result_root"))
    )
    return binding.Context(**fields)


class _Callbacks:
    def __init__(self, callbacks):
        self.binding_callbacks = callbacks
        self.state = None
        self.context = None

    def _request_context(self, request):
        if request["method"] in {"prepare", "check_applicability"}:
            self.context = _context(request)
        elif self.context is not None:
            for key in ("result_dir", "result_root"):
                if key in request:
                    self.context.output_dir = Path(request[key])
            self.context.observation_control = request.get("observation_control")

    def prepare(self):
        self.state = None
        self.state = _invoke(self.binding_callbacks["prepare"], self.context)

    def execute(self, budget, observation_control):
        self.context.budget = budget
        self.context.observation_control = observation_control
        return _invoke(self.binding_callbacks["execute"], self.state, self.context)

    def statistics(self, output):
        callback = self.binding_callbacks.get("statistics")
        return _invoke(callback, output, self.context) if callback else {}

    def write_result(self, directory, output):
        callback = self.binding_callbacks.get("result")
        value = _invoke(callback, output, self.context) if callback else output
        if isinstance(value, binding.Result):
            return _draft(directory, "result", value.data, value.files)
        return _draft(directory, "result", value)

    def reset(self):
        callback = self.binding_callbacks.get("reset")
        if callback is None:
            raise _worker.WorkerError(
                "unsupported_operation", "Reset is not implemented."
            )
        try:
            _invoke(callback, self.state, self.context)
        finally:
            self.state = None

    def shutdown(self):
        callback = self.binding_callbacks.get("shutdown")
        if callback:
            _invoke(callback, self.state, self.context)

    def check_applicability(self):
        return _invoke(self.binding_callbacks["check_applicability"], self.context)

    def execute_buffered(self, budget, trace):
        self.context.budget = budget
        self.context.observer = trace
        _invoke(self.binding_callbacks["execute_buffered"], self.state, self.context)

    def execute_stream(self, stream):
        self.context.budget = None
        self.context.stream = stream
        _invoke(self.binding_callbacks["execute_stream"], self.state, self.context)


def _evaluation(value, semantics=None):
    if isinstance(value, binding.Evaluation):
        outcome = _worker.Evaluation(value.metrics, value.verdict)
    elif not semantics or not (
        semantics.get("validity") or semantics.get("one_shot_completion")
    ):
        outcome = _worker.Evaluation(value, "not_required")
    else:
        raise ValueError("Scientific rules require an explicit Evaluation verdict.")
    if semantics is not None:
        completion = bool(semantics.get("one_shot_completion"))
        if not completion and outcome.verdict in {"reached", "not_reached"}:
            raise ValueError("A completion verdict requires declared completion rules.")
        if completion and outcome.verdict == "not_required":
            raise ValueError(
                "Declared completion rules require an explicit completion verdict."
            )
    return outcome


def _read_function_data(dataset):
    path = dataset / "data.json"
    if not path.exists() and not any(dataset.iterdir()):
        return {}
    data = read_json(path)
    if type(data) is not dict:
        raise TypeError("A function dataset must contain a JSON object in data.json.")
    return data


def serve_bindings(*, api_version, role, callbacks, metadata):
    if api_version != BINDING_API_VERSION:
        raise RuntimeError(
            f"Unsupported binding API version {api_version}; this CLI supports version 1."
        )
    callbacks = {
        name: callback for name, callback in callbacks.items() if callback is not None
    }
    initialize = None
    simple = callbacks.get("function")
    if simple is not None:
        file, name = simple
        selected = []

        def initialize():
            path = Path(file)
            sys.path.insert(0, str(path.parent))
            spec = importlib.util.spec_from_file_location("_metewand_benchmark", path)
            module = importlib.util.module_from_spec(spec)
            sys.modules[spec.name] = module
            spec.loader.exec_module(module)
            function = getattr(module, name, None)
            if not callable(function):
                raise ValueError(
                    f"{file}:{name} does not identify a callable function."
                )
            selected.append(function)

        if role == "implementation":
            callbacks = {
                "prepare": lambda context: _read_function_data(context.dataset_dir),
                "execute": lambda data, context: _invoke(selected[0], data, context),
                "reset": lambda state, context: None,
            }
        elif role == "dataset_materializer":
            callbacks = {"materialize": lambda context: _invoke(selected[0], context)}
        else:
            callbacks = {
                "evaluate": lambda context: _invoke(
                    selected[0],
                    _read_function_data(context.dataset_dir),
                    read_json(context.result_dir / "result.json")["data"],
                    context,
                )
            }
    allowed = (
        {
            "prepare",
            "execute",
            "result",
            "statistics",
            "reset",
            "shutdown",
            "check_applicability",
            "execute_buffered",
            "execute_stream",
        }
        if role == "implementation"
        else {"materialize"}
        if role == "dataset_materializer"
        else {"evaluate"}
    )
    if set(callbacks) - allowed or any(
        not callable(callback) for callback in callbacks.values()
    ):
        raise TypeError("Unsupported or noncallable worker callback.")
    if role == "implementation":
        if "prepare" not in callbacks or not (
            {"execute", "execute_buffered", "execute_stream"} & set(callbacks)
        ):
            raise TypeError(
                "Implementations require prepare and an execution callback."
            )
        worker = _Callbacks(callbacks)
    elif role == "dataset_materializer":

        def worker(source, parameters, seed, output):
            context = _context(
                dict(
                    method="materialize",
                    source_dir=source,
                    dataset_parameters=parameters,
                    dataset_seed=seed,
                    output_dir=output,
                )
            )
            value = _invoke(callbacks["materialize"], context)
            if isinstance(value, binding.Dataset):
                return _draft(output, "dataset", value.data, value.files)
            return _draft(output, "dataset", value)
    elif role == "problem_evaluator":

        def worker(dataset, contract, parameters, semantics, result, metrics):
            context = _context(
                dict(
                    method="evaluate",
                    dataset_dir=dataset,
                    problem_contract_id=contract,
                    problem_parameters=parameters,
                    problem_semantics=semantics,
                    result_dir=result,
                    metrics_path=metrics,
                )
            )
            return _evaluation(_invoke(callbacks["evaluate"], context), semantics)
    else:
        raise ValueError("Unknown worker role.")
    _worker._serve_socket(worker, role, metadata=metadata, initialize=initialize)
