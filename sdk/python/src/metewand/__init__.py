"""Small, process-based Python worker helpers for Metewand."""

from ._artifacts import (
    file_entry,
    read_json,
    write_dataset_manifest,
    write_json,
    write_metrics,
    write_result,
)
from ._buffered import BufferedTrace, Decision
from ._canonical import CanonicalJsonError, canonical_bytes, parse_json
from ._worker import (
    SDK_VERSION,
    Evaluation,
    Implementation,
    OneShotImplementation,
    BufferedImplementation,
    ProtocolError,
    WorkerError,
    serve_dataset_materializer,
    serve_evaluator,
    serve_implementation,
)

__version__ = SDK_VERSION

__all__ = [
    "BufferedImplementation",
    "BufferedTrace",
    "Decision",
    "CanonicalJsonError",
    "Evaluation",
    "Implementation",
    "OneShotImplementation",
    "ProtocolError",
    "WorkerError",
    "canonical_bytes",
    "file_entry",
    "parse_json",
    "read_json",
    "serve_dataset_materializer",
    "serve_evaluator",
    "serve_implementation",
    "write_dataset_manifest",
    "write_json",
    "write_metrics",
    "write_result",
]
