"""Run the fixed-dataset comparison with a prepared Nix package or app."""

import argparse
from pathlib import Path
import shutil
import subprocess


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("destination", type=Path)
parser.add_argument("--app", action="store_true")
args = parser.parse_args()
destination = args.destination.resolve()
example = Path(__file__).resolve().parent
shutil.copytree(example.parent / "fixed-dataset", destination)
for name in ("flake.nix", "flake.lock"):
    shutil.copyfile(example / name, destination / name)
system = subprocess.check_output(
    ["nix", "--extra-experimental-features", "nix-command flakes", "eval", "--raw",
     "--impure", "--expr", "builtins.currentSystem"], text=True
).strip()
attribute = "python3" if args.app else "python"
kind = "app" if args.app else "package"
output = "python3" if args.app else "out"
environment = (
    f"kind: nix\n    flake: .\n    installable: '.#{attribute}'\n"
    f"    system: {system}\n    output-kind: {kind}\n    output: {output}"
)
manifest = destination / "metewand.yaml"
manifest.write_text(manifest.read_text().replace('kind: "local"', environment))
for command in (
    ["lock"],
    ["lock", "--check"],
    ["run", "--locked", "--output-dir", str(destination.with_name(destination.name + "-results"))],
):
    subprocess.run(["metewand", *command], cwd=destination, check=True)
