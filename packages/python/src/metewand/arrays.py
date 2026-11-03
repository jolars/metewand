"""NumPy and SciPy artifacts without a dependency on either at import time."""

from pathlib import Path

from . import Artifact


def _numpy():
    try:
        import numpy
    except ImportError as error:
        raise ImportError("Install metewand[arrays] to use array artifacts.") from error
    return numpy


def _sparse():
    try:
        from scipy import sparse
    except ImportError as error:
        raise ImportError(
            "Install metewand[arrays] to use sparse array artifacts."
        ) from error
    return sparse


def _path(directory, name):
    name = str(name)
    if not name or name in {".", ".."} or any(c in name for c in "/\\:\0"):
        raise ValueError("array names must be relative file names")
    return Path(directory) / name


def write_array(directory, name, value):
    """Write a dense or sparse array and return its relative Artifact.

    ``name`` is a file stem. Dense arrays use .npy; sparse arrays use .npz and
    retain their sparse format. Object arrays are rejected before writing.
    """
    path = _path(directory, name)
    numpy = _numpy()
    if isinstance(value, numpy.ndarray):
        if value.dtype.hasobject:
            raise ValueError("object arrays cannot be stored without pickle")
        path = path.with_name(path.name + ".npy")
        numpy.save(path, value, allow_pickle=False)
    else:
        sparse = _sparse()
        if not sparse.issparse(value):
            raise TypeError("expected a NumPy or SciPy sparse array")
        path = path.with_name(path.name + ".npz")
        sparse.save_npz(path, value, compressed=False)
    return Artifact(path.name, "application/octet-stream")


def read_array(directory, artifact):
    """Read a .npy or sparse .npz artifact without permitting pickle."""
    path = _path(
        directory, artifact.path if isinstance(artifact, Artifact) else artifact
    )
    if path.suffix == ".npy":
        return _numpy().load(path, allow_pickle=False)
    if path.suffix == ".npz":
        return _sparse().load_npz(path)
    raise ValueError("expected a .npy or sparse .npz artifact")
