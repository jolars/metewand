"""Adapt the existing Benchopt dataset without changing its random-state policy."""

from metewand import serve_dataset_materializer
from upstream.simulated import Dataset


def materialize(context):
    data = Dataset(**context.parameters).get_data()
    return {"x": data["X"].tolist(), "y": data["y"].tolist()}


if __name__ == "__main__":
    serve_dataset_materializer(materialize)
