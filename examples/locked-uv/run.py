"""Run a Python comparison using an explicitly prepared uv environment."""

import argparse
from pathlib import Path
import shutil
import subprocess


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("destination", type=Path)
args = parser.parse_args()
destination = args.destination.resolve()
example = Path(__file__).resolve().parent
shutil.copytree(example, destination, ignore=shutil.ignore_patterns("__pycache__", ".venv"))
output = destination.with_name(destination.name + "-results")
for command in (
    ["lock"],
    ["lock", "--check"],
    ["run", "--locked", "--output-dir", str(output)],
    ["run", "--locked", "--output-dir", str(output), "--resume"],
    ["status", "--output-dir", str(output)],
):
    subprocess.run(["metewand", *command], cwd=destination, check=True)
