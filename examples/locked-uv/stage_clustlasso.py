"""Stage the existing clustlasso-cd analytic comparison for native uv locking."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess


WORKER = '''"""Run the packaged solvers on the existing analytic qualification problem."""

import json
import os
from pathlib import Path
import sys

if not os.environ.get("METEWAND_CLUSTERED_LASSO_LAUNCHED"):
    runtime = json.loads(Path(__file__).with_name("runtime.json").read_text())
    for name in ("OPENBLAS_NUM_THREADS", "OMP_NUM_THREADS", "MKL_NUM_THREADS", "BLIS_NUM_THREADS", "VECLIB_MAXIMUM_THREADS", "NUMEXPR_NUM_THREADS", "RAYON_NUM_THREADS"):
        os.environ[name] = "1"
    if runtime["ld_library_path"]:
        os.environ["LD_LIBRARY_PATH"] = runtime["ld_library_path"]
    os.environ["METEWAND_CLUSTERED_LASSO_LAUNCHED"] = "1"
    os.execv(sys.executable, [sys.executable, *sys.orig_argv[1:]])

import numpy as np
from clustlasso_cd.solvers import solve_pg, solve_hybrid
from metewand import serve_implementation


class Solver:
    def prepare(self, context):
        self.x = np.diag([1.0, 2.0, 3.0])
        self.y = np.array([-1.5, 1.05, 19 / 6])
        return self

    def execute(self, state, context):
        solve = solve_pg if sys.argv[1] == "pg" else solve_hybrid
        fit = solve(self.x, self.y, 0.1, 0.2, lipschitz=9.0, tolerance=1e-10,
                    max_iterations=1000, check_every=1, stopping_criterion="pg_residual")
        return {"coefficients": fit.beta.tolist(), "intercept": 0}


if __name__ == "__main__":
    solver = Solver()
    serve_implementation(prepare=solver.prepare, execute=solver.execute)
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clustlasso", required=True, type=Path)
    parser.add_argument("--destination", required=True, type=Path)
    parser.add_argument("--library-path", default=os.environ.get("LD_LIBRARY_PATH", ""))
    args = parser.parse_args()
    source = args.clustlasso.resolve()
    destination = args.destination.resolve()
    destination.mkdir(parents=True, exist_ok=False)
    package = destination / "package"
    package.mkdir()
    files = subprocess.check_output(["git", "ls-files", "-z", "--", "pyproject.toml", "uv.lock", ".python-version", "Cargo.toml", "Cargo.lock", "README.md", "rust-toolchain.toml", "rust-toolchain", ".cargo", "build.rs", "rust", "src"], cwd=source).split(b"\0")
    for filename in filter(None, files):
        relative = Path(os.fsdecode(filename))
        target = package / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source / relative, target)
    repository = Path(__file__).resolve().parents[2]
    adapters = repository / "examples/adopters"
    shutil.copyfile(adapters / "clustlasso-problem.yaml", destination / "problem.yaml")
    shutil.copyfile(adapters / "evaluator.py", destination / "evaluator.py")
    shutil.copytree(repository / "packages/python/src/metewand", destination / "metewand", ignore=shutil.ignore_patterns("__pycache__"))
    (destination / "worker.py").write_text(WORKER)
    (destination / "runtime.json").write_text(json.dumps({"ld_library_path": args.library_path}) + "\n")
    references = destination / "reference"
    references.mkdir()
    import importlib.util
    spec = importlib.util.spec_from_file_location("qualification_evaluator", adapters / "evaluator.py")
    evaluator = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(evaluator)
    result = {"coefficients": [-1, 0.5, 1], "intercept": 0}
    for name, data in (("result", result), ("metrics", evaluator.metrics("clustlasso", result))):
        envelope = {"schema_version": 1, "data": data}
        if name == "result": envelope["files"] = []
        (references / f"{name}.json").write_text(json.dumps(envelope) + "\n")
    (destination / "metewand.yaml").write_text('''version: 1
name: clustlasso-locked-uv
problems:
  clustered_lasso:
    contract: problem.yaml
    evaluator:
      runner: python
      protocol-transport: socket
      entrypoint: evaluator.py
      args: [clustlasso]
      sources: [evaluator.py, "metewand/*.py"]
      environment: python
implementations:
  pg: &solver
    runner: python
    protocol-transport: socket
    entrypoint: worker.py
    args: [pg]
    sources: [worker.py, runtime.json, "metewand/*.py"]
    environment: python
    problem-contracts: [clustered_lasso]
    capabilities: [one_shot]
  hybrid:
    <<: *solver
    args: [hybrid]
environments:
  python:
    kind: uv
    project: package
    lockfile: package/uv.lock
    groups: [benchmark]
experiments:
  - name: analytic
    problem: clustered_lasso
    implementations: [pg, hybrid]
    measurement-repetitions: 2
    seed: 3
''')
    print(destination)


if __name__ == "__main__":
    main()
