"""Adapt the existing Benchopt dataset without changing its random-state policy."""

from metewand import (
    file_entry,
    serve_dataset_materializer,
    write_dataset_manifest,
    write_json,
)
from upstream.simulated import Dataset


def materialize(source_dir, dataset_parameters, seed, output_dir):
    data = Dataset(**dataset_parameters).get_data()
    write_json(output_dir / "data.json", {
        "x": data["X"].tolist(), "y": data["y"].tolist(),
    })
    return write_dataset_manifest(
        output_dir, [file_entry(output_dir, "data.json", "application/json")]
    )


if __name__ == "__main__":
    serve_dataset_materializer(materialize)
