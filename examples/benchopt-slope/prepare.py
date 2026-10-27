"""Stage adapters, the package, and unchanged scientific sources into a new directory."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def prepare(benchmark, destination):
    example = Path(__file__).resolve().parent
    sources = {
        "datasets/simulated.py": "simulated.py",
        "objective.py": "objective.py",
        "solvers/python_pgd.py": "python_pgd.py",
        "solvers/admm.py": "admm.py",
    }
    # Read first so a missing scientific source cannot leave a partial benchmark.
    contents = {path: (benchmark / path).read_bytes() for path in sources}
    revision = subprocess.check_output(
        ["git", "-C", str(benchmark), "rev-parse", "HEAD"], text=True,
    ).strip()
    status = subprocess.check_output(
        ["git", "-C", str(benchmark), "status", "--porcelain", "--", *sources],
        text=True,
    ).strip()
    destination.mkdir()
    for name in ("metewand.yaml", "dataset.py", "solver.py", "evaluator.py",
                 "test_science.py", "validate.py"):
        shutil.copyfile(example / name, destination / name)
    shutil.copytree(example / "reference", destination / "reference")
    shutil.copytree(example.parents[1] / "packages/python/src/metewand",
                    destination / "metewand", ignore=shutil.ignore_patterns("__pycache__"))
    (destination / "upstream").mkdir()
    for path, content in contents.items():
        (destination / "upstream" / sources[path]).write_bytes(content)
    origins = {
        "repository": "https://github.com/jolars/benchmark_slope",
        "revision": revision,
        "modified_sources": bool(status),
        "files": {path: hashlib.sha256(content).hexdigest()
                  for path, content in contents.items()},
    }
    (destination / "origins.json").write_text(json.dumps(origins, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--benchmark", required=True, type=Path)
    parser.add_argument("--destination", required=True, type=Path)
    args = parser.parse_args()
    prepare(args.benchmark.resolve(), args.destination.resolve())
