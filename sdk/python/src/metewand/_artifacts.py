"""Explicit file inventories and version-1 worker output envelopes."""

import hashlib
import re
from pathlib import Path

from ._canonical import MAX_SAFE_INTEGER, canonical_bytes, parse_json


def read_json(path):
    """Read a document using the restricted version-1 JSON domain."""
    return parse_json(Path(path).read_bytes())


def write_json(path, value):
    """Write canonical JSON and close the file before returning."""
    encoded = canonical_bytes(value)
    Path(path).write_bytes(encoded)


def _relative_path(value):
    if not isinstance(value, (str, Path)):
        raise TypeError("Paths must be strings or pathlib Paths.")
    value = str(value)
    if (
        not value
        or value.startswith("/")
        or "\\" in value
        or "\x00" in value
        or any(part in {"", ".", ".."} for part in value.split("/"))
    ):
        raise ValueError(
            "Paths must be normalized, nonempty, and relative to the assigned directory."
        )
    return value


def _output_path(directory, relative):
    relative = _relative_path(relative)
    root = Path(directory).resolve()
    target = root / relative
    if not target.resolve().is_relative_to(root):
        raise ValueError("Output paths must stay beneath the assigned directory.")
    return target


def _object(value, name):
    if type(value) is not dict:
        raise ValueError(f"{name} must be a JSON object.")
    canonical_bytes(value)
    return value


def _representation(value):
    _object(value, "File representation")
    if not value or set(value) - {"shape", "data_type", "byte_order", "sparse_layout"}:
        raise ValueError(
            "File representation must contain only version-1 representation fields."
        )
    if "shape" in value and (
        type(value["shape"]) is not list
        or any(
            type(size) is not int or not 0 <= size <= MAX_SAFE_INTEGER
            for size in value["shape"]
        )
    ):
        raise ValueError("Shape must contain nonnegative safe integers.")
    for key in ("data_type", "sparse_layout"):
        if key in value and (type(value[key]) is not str or not value[key]):
            raise ValueError(f"{key} must be a nonempty string.")
    if "byte_order" in value and (
        type(value["byte_order"]) is not str
        or value["byte_order"] not in {"little", "big", "not_applicable"}
    ):
        raise ValueError("Invalid byte order.")


def file_entry(directory, path, media_type, *, representation=None):
    """Hash a completed file and describe its author-supplied representation."""
    path = _relative_path(path)
    target = _output_path(directory, path)
    if not target.is_file() or target.is_symlink():
        raise ValueError("An inventory entry must identify a regular file.")
    with target.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    entry = {
        "path": path,
        "media_type": media_type,
        "sha256": digest,
        "size_bytes": target.stat().st_size,
    }
    if representation is not None:
        entry["representation"] = representation
    _files([entry], manifest=None)
    return entry


def _files(files, manifest):
    entries = list(files)
    paths = set()
    for entry in entries:
        _object(entry, "File entry")
        required = {"path", "media_type", "sha256", "size_bytes"}
        if not required <= set(entry) or set(entry) - required - {"representation"}:
            raise ValueError(
                "File entries must contain the version-1 inventory fields."
            )
        if type(entry["path"]) is not str:
            raise ValueError("File inventory paths must be strings.")
        path = _relative_path(entry["path"])
        if path in paths or path == manifest:
            raise ValueError(
                "File inventory paths must be unique and exclude the manifest itself."
            )
        paths.add(path)
        if type(entry["media_type"]) is not str or not entry["media_type"]:
            raise ValueError("Media types must be nonempty strings.")
        if type(entry["sha256"]) is not str or not re.fullmatch(
            "[0-9a-f]{64}", entry["sha256"]
        ):
            raise ValueError("File digests must be lowercase SHA-256 hex strings.")
        size = entry["size_bytes"]
        if type(size) is not int or not 0 <= size <= MAX_SAFE_INTEGER:
            raise ValueError("File sizes must be nonnegative safe integers.")
        if "representation" in entry:
            _representation(entry["representation"])
    return sorted(entries, key=lambda entry: entry["path"].encode("utf-8"))


def write_dataset_manifest(directory, files, *, manifest="dataset-manifest.json"):
    """Write a complete dataset inventory last and return its relative path."""
    manifest = _relative_path(manifest)
    document = {"schema_version": 1, "complete": True, "files": _files(files, manifest)}
    write_json(_output_path(directory, manifest), document)
    return manifest


def write_result(directory, data, *, schema=None, files=(), manifest="result.json"):
    """Write a problem-owned result envelope and return its relative manifest path."""
    manifest = _relative_path(manifest)
    document = {
        "schema_version": 1,
        "data": _object(data, "Result data"),
        "files": _files(files, manifest),
    }
    if schema is not None:
        document["schema"] = _relative_path(schema)
    write_json(_output_path(directory, manifest), document)
    return manifest


def write_metrics(path, data, *, schema=None):
    """Write evaluator-owned metrics at the exact path assigned by Metewand."""
    document = {"schema_version": 1, "data": _object(data, "Metrics data")}
    if schema is not None:
        document["schema"] = _relative_path(schema)
    write_json(path, document)
