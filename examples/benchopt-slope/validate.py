"""Validate the staged comparison through the public CLI and preserve its evidence."""

import argparse
from collections import defaultdict
import json
import math
from pathlib import Path
import re
from statistics import median
import subprocess
import sys


def validate(metewand, output_dir):
    root = Path(__file__).resolve().parent
    export_dir = output_dir.with_name(output_dir.name + "-export")
    evidence_dir = output_dir.with_name(output_dir.name + "-validation")
    for path in (output_dir, export_dir, evidence_dir):
        if path.exists() or path.is_symlink():
            raise FileExistsError(f"Validation requires a fresh destination: {path}")
    manifest = (root / "metewand.yaml").read_text()
    assert not (root / "schemas").exists()
    assert not (root / "problem.yaml").exists()
    assert not re.search(r"^\s+(?:contract|semantics-schema|parameter-schema):", manifest, re.M)
    for name in ("dataset.py", "solver.py", "evaluator.py"):
        assert "METEWAND_PROTOCOL" not in (root / name).read_text()
    evidence_dir.mkdir()

    science_args = [sys.executable, "-B", "-m", "unittest", "-v", "test_science"]
    science = subprocess.run(science_args, cwd=root, capture_output=True, text=True, check=False)
    (evidence_dir / "science.txt").write_text(science.stdout + science.stderr)
    (evidence_dir / "commands.jsonl").write_text(json.dumps({
        "argv": science_args, "exit_code": science.returncode,
    }) + "\n")
    assert science.returncode == 0, science.stdout + science.stderr

    def command(label, *arguments, expected=0, human=False):
        args = [str(metewand), *map(str, arguments), "--output", "human" if human else "json"]
        result = subprocess.run(args, cwd=root, capture_output=True, text=True, check=False)
        suffix = ".txt" if human else ".json"
        (evidence_dir / (label + suffix)).write_text(result.stdout)
        if result.stderr:
            (evidence_dir / (label + ".stderr")).write_text(result.stderr)
        with (evidence_dir / "commands.jsonl").open("a") as receipt:
            receipt.write(json.dumps({"argv": args, "exit_code": result.returncode}) + "\n")
        assert result.returncode == expected, f"{label}: {result.stdout}\n{result.stderr}"
        if human:
            return result.stdout
        envelope = json.loads(result.stdout)
        assert envelope["schema_version"] == 1
        return envelope["data"]

    checked = command("check", "check", "--workers")
    assert len(checked["reference_cases"]) == 1
    assert checked["reference_cases"][0]["metrics"]["rel_duality_gap"] == 0
    plan = command("plan", "plan")
    candidates = plan["experiments"][0]["candidates"]
    assert len(candidates) == 4 and plan["summary"]["attempt_slots"] == 12
    definitions = {item["id"]: item["name"] for item in plan["definitions"]["implementations"]}
    contract = command("contract", "explain", plan["resources"]["problem_contracts"][0]["id"])
    semantics = contract["subject"]["record"]["value"]["semantics"]
    target = semantics["one_shot_completion"]["rel_duality_gap"]
    expected_problem = {"reg": 0.1, "q": 0.1, "fit_intercept": False}
    by_candidate, by_slot = {}, {}
    for index, candidate in enumerate(candidates):
        record = command(f"configuration-{index}", "explain", candidate["implementation_configuration"])
        method = definitions[record["subject"]["record"]["definition"]]
        case = candidate["source"]["case"]
        assert method in {"pgd", "admm"} and case in {"converged", "iteration-limit"}
        parameters = candidate["parameters"]
        assert parameters["problem"] == expected_problem
        assert parameters["dataset"]["random_state"] == 27
        assert parameters["implementation"]["iterations"] == (1000 if case == "converged" else 1)
        specifications = candidate["specifications"]
        assert len(specifications) == 1 and len(specifications[0]["attempt_slots"]) == 3
        by_candidate[candidate["id"]] = (method, case, parameters)
        for slot in specifications[0]["attempt_slots"]:
            assert slot not in by_slot
            by_slot[slot] = candidate["id"]
    assert {(method, case) for method, case, _ in by_candidate.values()} == {
        (method, case) for method in ("pgd", "admm") for case in ("converged", "iteration-limit")
    }
    human_plan = command("plan-human", "plan", human=True)
    assert "implementation parameters:" in human_plan and "prepare_and_execute" in human_plan

    run = command("run", "run", "--output-dir", output_dir, expected=5)
    attempts = run["attempts"]
    assert len(attempts) == 12, run["summary"]
    assert run["summary"]["accepted_attempts"] == run["summary"]["failed_attempts"] == 6, run["summary"]
    assert {attempt["logical_slot_id"] for attempt in attempts} == set(by_slot)
    for attempt in attempts:
        _, case, _ = by_candidate[by_slot[attempt["logical_slot_id"]]]
        reached = case == "converged"
        assert attempt["status"] == ("accepted" if reached else "failed")
        assert attempt["observation_id"] is not None
        assert math.isfinite(attempt["metrics"]["rel_duality_gap"])
        assert (attempt["metrics"]["rel_duality_gap"] <= target) == reached
        if reached:
            assert attempt["diagnostic"] is None
        else:
            assert attempt["diagnostic"]["code"] == "completion_target_not_reached"

    status = command("status", "status", "--output-dir", output_dir)
    assert len(status["observations"]) == 12
    assert status["summary"]["satisfied_slots"] == status["summary"]["retry_required_slots"] == 6
    assert status["summary"]["selected_observations"] == 6
    for index, observation in enumerate(status["observations"]):
        assert observation["valid"] is True
        assert observation["selected"] == (observation["completion"] == "reached")
        explained = command(f"observation-{index}", "explain", observation["id"], "--output-dir", output_dir)
        document = explained["subject"]["record"]["document"]
        assert document["validity"]["valid"] is True
        assert document["validity"]["completion"] == observation["completion"]
    diagnostic_ids = {attempt["diagnostic_id"] for attempt in status["attempts"]
                      if attempt["diagnostic_id"] is not None}
    assert len(diagnostic_ids) == 1
    diagnostic_id = next(iter(diagnostic_ids))
    diagnostic = command("diagnostic", "explain", diagnostic_id, "--output-dir", output_dir)
    assert diagnostic["subject"]["record"]["code"] == "completion_target_not_reached"
    assert "valid result" in diagnostic["subject"]["record"]["message"]
    human_status = command("status-human", "status", "--output-dir", output_dir, human=True)
    assert "completion_target_not_reached" in human_status
    human_observation = command("observation-human", "explain", status["observations"][0]["id"],
                                "--output-dir", output_dir, human=True)
    assert "timed_wall_time_ns" in human_observation and "implementation_parameters" in human_observation
    command("diagnostic-human", "explain", diagnostic_id, "--output-dir", output_dir, human=True)

    command("export", "export", "--output-dir", output_dir, "--destination", export_dir)
    rows = [json.loads(line) for line in (export_dir / "results.jsonl").read_text().splitlines()]
    observations = [row for row in rows if row["record_type"] == "observation"]
    assert len({row["id"] for row in observations}) == 12
    assert len(observations) == 36
    assert len({row["provenance"]["data"]["context"]["scheduling"]
                ["comparison_block"]["dataset_instance"] for row in observations}) == 1
    grouped = defaultdict(list)
    for row in observations:
        assert row["outcome"] != "invalid" and row["valid"] is True
        if row["metric_name"] != "/rel_duality_gap":
            continue
        method, case, parameters = by_candidate[row["candidate_id"]]
        provenance = row["provenance"]["data"]
        assert provenance["execution"]["problem_parameters"] == parameters["problem"]
        assert provenance["execution"]["implementation_parameters"] == parameters["implementation"]
        assert provenance["context"]["dataset_materialization"]["derivation"]["parameters"] == parameters["dataset"]
        assert row["compatibility"]["timing_scope"] == "prepare_and_execute"
        assert row["compatibility"]["primary_time"] == "timed_wall_time"
        assert row["timed_wall_time_status"] == "complete"
        for timing in ("timed_wall_time_ns", "implementation_time_ns", "evaluation_time_ns"):
            assert isinstance(row[timing], int) and row[timing] > 0
        reached = case == "converged"
        assert row["completion"] == ("reached" if reached else "not_reached")
        assert row["selected"] == reached
        assert row["failure_code"] == (None if reached else "completion_target_not_reached")
        grouped[method, case].append(row)

    summary = []
    for (method, case), entries in sorted(grouped.items()):
        assert len(entries) == 3
        indices = [entry["provenance"]["data"]["context"]["scheduling"]["comparison_block"]
                   ["measurement_index"] for entry in entries]
        assert sorted(indices) == [0, 1, 2]
        item = {"method": method, "case": case, "repetitions": len(entries),
                "relative_gaps": [entry["metric_value"] for entry in entries]}
        for field in ("timed_wall_time", "implementation_time", "evaluation_time"):
            item[f"median_{field}_ms"] = round(median(entry[field + "_ns"] for entry in entries) / 1e6, 6)
        summary.append(item)
    assert len(summary) == 4
    (evidence_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print("Validated 12 attempts: 6 accepted, 6 completion misses, and 12 valid observations.")
    for item in summary:
        gaps = ", ".join(f"{gap:.3g}" for gap in item["relative_gaps"])
        print(f"{item['method']:4} {item['case']:15} gaps=[{gaps}] "
              f"median wall/solve/eval ms={item['median_timed_wall_time_ms']:.3f}/"
              f"{item['median_implementation_time_ms']:.3f}/{item['median_evaluation_time_ms']:.3f}")
    print(f"Evidence: {evidence_dir}\nTable: {export_dir / 'results.jsonl'}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metewand", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    validate(args.metewand.resolve(strict=True), args.output_dir.resolve())
