"""Refresh the preprocessing declaration in an existing package smoke bundle."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    shutil.copytree(args.bundle, args.output_dir)
    contract = args.output_dir / "problem.yaml"
    original = contract.read_text()
    marker = '"semantics":\n'
    if '"preprocessing"' not in original:
        assert original.count(marker) == 1
        contract.write_text(original.replace(marker, marker + '  "preprocessing": {}\n'))
    schema_path = args.output_dir / "schemas/semantics.json"
    schema = json.loads(schema_path.read_text())
    schema["properties"].setdefault("preprocessing", {"type": "object", "additionalProperties": False})
    schema_path.write_text(json.dumps(schema, indent=2) + "\n")
    changes = {}
    for name in ("problem.yaml", "schemas/semantics.json"):
        changes[name] = {"original_sha256": hashlib.sha256((args.bundle / name).read_bytes()).hexdigest(), "staged_sha256": hashlib.sha256((args.output_dir / name).read_bytes()).hexdigest()}
    (args.output_dir / "adapter-refresh.json").write_text(json.dumps({"schema_version": 1, "source_bundle": str(args.bundle.resolve()), "changes": changes}, indent=2) + "\n")


if __name__ == "__main__":
    main()
