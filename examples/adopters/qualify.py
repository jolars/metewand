"""Exercise the public CLI, including a killed owner and a durable resume."""

import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metewand", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    runner, manifest, root = args.metewand.resolve(), args.manifest.resolve(), args.output_dir.resolve()
    root.mkdir()
    commands = []

    def invoke(label, *arguments):
        command = [str(runner), *map(str, arguments), "--output", "json"]
        started = time.perf_counter_ns()
        result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        commands.append({"label": label, "command": command, "exit_code": result.returncode, "elapsed_wall_time_ns": time.perf_counter_ns() - started})
        (root / f"{label}.json").write_bytes(result.stdout)
        (root / f"{label}.stderr").write_bytes(result.stderr)
        if result.returncode:
            raise RuntimeError(f"{label}: {result.stderr.decode()}")
        return json.loads(result.stdout)["data"]

    invoke("check", "check", "--manifest", manifest, "--workers")
    plan = invoke("plan", "plan", "--manifest", manifest)
    candidate = plan["experiments"][0]["candidates"][0]["id"]
    invoke("selected", "run", "--manifest", manifest, "--output-dir", root / "selected", "--select", candidate, "--max-attempts", 1)
    invoke("prefix", "run", "--manifest", manifest, "--output-dir", root / "run", "--max-attempts", 1)
    before = invoke("before", "status", "--output-dir", root / "run")
    retained = {o["id"] for o in before["observations"] if o["selected"]}
    assert retained, "The durable prefix must contain an observation."
    command = [str(runner), "run", "--manifest", str(manifest), "--output-dir", str(root / "run"), "--resume", "--output", "jsonl"]
    with (root / "interrupted.stderr").open("wb") as errors, (root / "interrupted.jsonl").open("w") as events:
        process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=errors, text=True)
        interrupted = False
        try:
            for line in process.stdout:
                events.write(line)
                event = json.loads(line)
                if event["data"].get("event") != "attempt_started":
                    continue
                # A worker child proves execution passed reservation and launch.
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline and process.poll() is None:
                    children = Path(f"/proc/{process.pid}/task/{process.pid}/children").read_text().split()
                    if children:
                        process.kill()
                        for child in children:
                            try:
                                os.kill(int(child), signal.SIGKILL)
                            except ProcessLookupError:
                                pass
                        interrupted = True
                        break
                    time.sleep(0.0005)
                if interrupted:
                    break
        finally:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=10)
    assert interrupted, "The comparison ended before a live worker could be interrupted."
    commands.append({"label": "interrupted", "command": command, "exit_code": process.returncode, "signal": "SIGKILL", "live_worker_observed": True})
    deadline = time.monotonic() + 40
    while True:
        status = invoke("interrupted-status", "status", "--output-dir", root / "run")
        if any(a["recovery_required"] for a in status["attempts"]):
            break
        if time.monotonic() >= deadline:
            raise RuntimeError("The interrupted owner's lease did not expire.")
        time.sleep(1)
    invoke("resume", "run", "--manifest", manifest, "--output-dir", root / "run", "--resume", "--retry")
    after = invoke("status", "status", "--output-dir", root / "run")
    assert retained <= {o["id"] for o in after["observations"] if o["selected"]}
    assert after["summary"]["pending_slots"] == after["summary"]["active_slots"] == after["summary"]["retry_required_slots"] == 0
    assert after["summary"]["failed_attempts"] >= 1
    invoke("explain", "explain", sorted(retained)[0], "--output-dir", root / "run")
    finished = invoke("complete-resume", "run", "--manifest", manifest, "--output-dir", root / "run", "--resume", "--retry")
    assert not finished["attempts"], "A completed resume must allocate no attempts."
    for encoding in ("jsonl", "csv"):
        invoke(f"export-{encoding}", "export", "--output-dir", root / "run", "--format", encoding, "--destination", root / encoding)
    rows = [json.loads(line) for line in (root / "jsonl/results.jsonl").read_text().splitlines()]
    csv.field_size_limit(sys.maxsize)
    with (root / "csv/results.csv").open(newline="") as stream:
        csv_rows = list(csv.DictReader(stream))
    assert len(rows) == len(csv_rows)
    for row, csv_row in zip(rows, csv_rows):
        for field in ("profile", "provenance", "evidence", "compatibility", "timing", "metric_value", "provisional_metric_value"):
            assert row.get(field) == (json.loads(csv_row[field]) if csv_row[field] else None), field
        if row["record_type"] == "observation":
            assert row["logical_run_id"] and row["resolved_run_id"] and row["logical_run_id"] != row["resolved_run_id"]
            assert row["compatibility"]["environment_class"] == "development"
            assert row["compatibility"]["timing_scope"] == "prepare_and_execute"
            assert row["metric_value"] is not None
    report = {
        "schema_version": 1,
        "manifest": str(manifest),
        "manifest_sha256": hashlib.sha256(manifest.read_bytes()).hexdigest(),
        "candidates": {candidate["id"]: {"implementation_configuration": candidate["implementation_configuration"], "parameters": candidate["parameters"]} for experiment in plan["experiments"] for candidate in experiment["candidates"]},
        "commands": commands,
        "interruption": {"live_worker_observed": True, "retained_observation_ids": sorted(retained)},
        "summary": after["summary"],
        "table_hashes": {kind: hashlib.sha256((root / f"{kind}/results.{kind}").read_bytes()).hexdigest() for kind in ("jsonl", "csv")},
        "observations": [{key: row[key] for key in ("id", "attempt_id", "candidate_id", "logical_run_id", "resolved_run_id", "metric_name", "metric_value", "timed_wall_time_ns", "evaluation_time_ns", "compatibility_id", "compatibility", "profile") if key in row} for row in rows if row["record_type"] == "observation"],
        "failures": [{"attempt_id": row["attempt_id"], "failure_code": row["failure_code"], "outcome": row["outcome"]} for row in rows if row["record_type"] == "attempt" and row["outcome"] != "accepted"],
    }
    report.update(provenance(rows))
    (root / "qualification.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"output_dir": str(root), "summary": report["summary"]}))


def provenance(rows):
    executables, hosts = {}, {}
    for row in rows:
        if row["record_type"] != "observation":
            continue
        context = row["provenance"]["data"]["context"]
        for role, executable in [("runner", context["runner"]), ("implementation", context["implementation"]["executable"]), ("evaluator", context["evaluator"]["executable"])]:
            key = executable["executable_sha256"]
            executables.setdefault(key, {"roles": [], "evidence": executable})
            if role not in executables[key]["roles"]:
                executables[key]["roles"].append(role)
        host = context["host_observation"]
        hosts[json.dumps(host, sort_keys=True)] = host
    return {"executables": executables, "host_observations": list(hosts.values())}


if __name__ == "__main__":
    main()
