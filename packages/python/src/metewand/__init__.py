"""Stable scientific callbacks and values for the Metewand CLI."""

from dataclasses import dataclass, field
import importlib.util
from importlib.metadata import PackageNotFoundError, version
import json
import os
from pathlib import Path
import sys

try:
    __version__ = version("metewand")
except PackageNotFoundError:
    __version__ = "0+source"
BINDING_API_VERSION = 1


class Context:
    """Callback inputs supplied by the CLI; fields may grow within API version 1."""

    def __init__(self, **fields):
        self.__dict__.update(fields)


@dataclass(frozen=True)
class Artifact:
    """A completed file relative to the callback's assigned output directory."""

    path: str | Path
    media_type: str
    representation: dict | None = None


@dataclass(frozen=True)
class Dataset:
    """Generated JSON data, declared files, or both."""

    data: dict | None = None
    files: tuple | list = ()


@dataclass(frozen=True)
class Result:
    """Scientific result data and optional completed files."""

    data: dict = field(default_factory=dict)
    files: tuple | list = ()


@dataclass(frozen=True)
class Evaluation:
    """Independent scientific metrics and an explicit verdict."""

    metrics: dict
    verdict: str


class WorkerError(Exception):
    """A callback failure interpreted and validated by the CLI."""

    def __init__(self, code, message, details=None):
        super().__init__(message)
        self.code = code
        self.details = details


def _unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError(f"Duplicate JSON object key: {key!r}.")
        value[key] = item
    return value


def _reject_constant(text):
    raise ValueError(f"Invalid JSON constant: {text}")


def read_json(path):
    """Read UTF-8 JSON without simplifying arrays or accepting duplicate keys."""
    return json.loads(Path(path).read_text(encoding="utf-8"), object_pairs_hook=_unique_object,
                      parse_constant=_reject_constant)


def write_json(path, value):
    """Write ordinary UTF-8 JSON; the CLI owns canonical artifact formats."""
    Path(path).write_text(json.dumps(value, ensure_ascii=False, allow_nan=False,
                                    separators=(",", ":")), encoding="utf-8")


def _serve(role, callbacks):
    supplied = os.environ.get("METEWAND_ADAPTER_PATH")
    if not supplied:
        raise RuntimeError("Run this worker through the Metewand CLI to supply its adapter.")
    path = Path(supplied)
    spec = importlib.util.spec_from_file_location(
        "_metewand_cli_adapter", path,
        submodule_search_locations=[str(path.parent)] if path.name == "__init__.py" else None,
    )
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Cannot load the Metewand CLI adapter at {path}.")
    adapter = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = adapter
    spec.loader.exec_module(adapter)
    if getattr(adapter, "BINDING_API_VERSION", None) != BINDING_API_VERSION:
        raise RuntimeError("The Metewand CLI does not support binding API version 1; update the CLI.")
    return adapter.serve_bindings(api_version=BINDING_API_VERSION, role=role,
                                  callbacks=callbacks, metadata={"name": "metewand-python", "version": __version__})


def serve_implementation(**callbacks):
    """Serve named lifecycle callbacks with explicit state and a context."""
    return _serve("implementation", callbacks)


def serve_dataset_materializer(materialize):
    """Serve materialize(context), returning a Dataset or a JSON object."""
    return _serve("dataset_materializer", {"materialize": materialize})


def serve_evaluator(evaluate):
    """Serve evaluate(context), returning an Evaluation."""
    return _serve("problem_evaluator", {"evaluate": evaluate})


__all__ = ["BINDING_API_VERSION", "Artifact", "Context", "Dataset", "Evaluation", "Result",
           "WorkerError", "read_json", "write_json", "serve_implementation",
           "serve_dataset_materializer", "serve_evaluator"]
