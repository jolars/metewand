"""Stage adapters around existing adopter code without editing either repository."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tomllib

from evaluator import metrics as evaluate_metrics

HERE = Path(__file__).resolve().parent
REPOSITORY = HERE.parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--basin", required=True, type=Path)
    parser.add_argument("--clustlasso", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    stage = args.output_dir.resolve()
    stage.mkdir()
    source = stage / "native/src"
    source.mkdir(parents=True)
    for name in ("basin.rs", "clustlasso.rs", "bridge.rs"):
        text = (HERE / name).read_text()
        if name != "bridge.rs":
            modules = json.dumps(str(REPOSITORY / "src/modules.rs"), ensure_ascii=False)
            include = f"include!({modules});\n\n"
            if text.startswith("#!["):
                attribute, text = text.split("\n", 1)
                text = attribute + "\n\n" + include + text
            else:
                text = include + text
        (source / name).write_text(text)
    for path in (args.clustlasso / "rust").glob("*.rs"):
        if path.name not in ("lib.rs", "overlap.rs"):
            shutil.copy2(path, source / path.name)
    trajectory = source / "trajectory.rs"
    text = trajectory.read_text()
    original = "    pub(crate) fn expired(&self) -> bool {\n"
    assert text.count(original) == 1, "The native recorder changed; review the bridge."
    text = text.replace(original, original + "        if crate::bridge::stopped() { return true; }\n")
    original = "        let mut ordered = beta.to_vec();\n"
    assert text.count(original) == 1, "The native observation point changed; review the bridge."
    text = text.replace(original, "        if crate::bridge::record(iteration, beta, epsilon_g, work) { return; }\n" + original)
    trajectory.write_text(text)
    dependencies = {
        "basin": args.basin.resolve() / "crates/basin",
        "competitor-bench": args.basin.resolve() / "crates/competitor-bench",
    }
    # The adapters compile private worker helpers, so they inherit their
    # dependencies without introducing a Metewand library package.
    metewand = (REPOSITORY / "Cargo.toml").read_text()
    rust_version = tomllib.loads(metewand)["package"]["rust-version"]
    manifest = f'[package]\nname = "metewand-adopters"\nversion = "0.1.0"\nedition = "2024"\nrust-version = "{rust_version}"\npublish = false\n[workspace]\n[dependencies]\n'
    manifest += "".join(f'{name} = {{ path = {json.dumps(str(path))} }}\n' for name, path in dependencies.items())
    manifest += metewand.split("[dependencies]\n", 1)[1].split("[dev-dependencies]", 1)[0]
    manifest += '\n[[bin]]\nname = "basin-worker"\npath = "src/basin.rs"\n[[bin]]\nname = "clustlasso-worker"\npath = "src/clustlasso.rs"\n'
    (stage / "native/Cargo.toml").write_text(manifest)
    for adopter in ("basin", "clustlasso"):
        root = stage / adopter
        root.mkdir()
        shutil.copy2(HERE / f"{adopter}.yaml", root / "metewand.yaml")
        shutil.copy2(HERE / f"{adopter}-problem.yaml", root / "problem.yaml")
        shutil.copy2(HERE / "evaluator.py", root / "evaluator.py")
        shutil.copytree(REPOSITORY / "packages/python/src/metewand", root / "metewand", ignore=shutil.ignore_patterns("__pycache__"))
        (root / "reference").mkdir()
        if adopter == "basin":
            result, metrics = {"x": [1, 1], "local_objective": 0}, {"objective": 0, "parameter_error": 0}
        else:
            result = {"coefficients": [-1, 0.5, 1], "intercept": 0}
            metrics = evaluate_metrics("clustlasso", result)
        for name, value in [("result", result), ("metrics", metrics)]:
            document = {"schema_version": 1, "data": value}
            if name == "result":
                document["files"] = []
            (root / f"reference/{name}.json").write_text(json.dumps(document) + "\n")
    (stage / ".gitignore").write_text("native/target/\n**/worker\n**/*.metewand-build.json\n**/run*/\n**/jsonl/\n**/csv/\n__pycache__/\n")
    subprocess.run(["git", "init", "-q", str(stage)], check=True)
    print(stage)


if __name__ == "__main__":
    main()
