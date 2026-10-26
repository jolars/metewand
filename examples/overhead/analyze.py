"""Keep every overhead repetition and summarize only compatible timing scopes."""

import argparse
from collections import defaultdict
import hashlib
import json
from pathlib import Path
from statistics import median


def analyze(plan, rows):
    settings = {
        candidate["id"]: candidate["parameters"]["implementation"]
        for experiment in plan["data"]["experiments"]
        for candidate in experiment["candidates"]
    }
    records, groups = [], defaultdict(list)
    for row in rows:
        if row["record_type"] != "observation":
            continue
        statistics = row["provenance"]["data"]["implementation_statistics"]
        context = row["provenance"]["data"]["context"]
        parameters = settings[row["candidate_id"]]
        measured = {
            "observation_id": row["id"],
            "attempt_id": row["attempt_id"],
            "logical_run_id": row["logical_run_id"],
            "resolved_run_id": row["resolved_run_id"],
            "measurement_index": context["scheduling"]["comparison_block"]["measurement_index"],
            "timing_scope": row["compatibility"]["timing_scope"],
            "environment_class": row["compatibility"]["environment_class"],
            "control_class": row["compatibility"]["control_class"],
            "compatibility_id": row["compatibility_id"],
            "evaluator_metrics": row["metrics"],
            "settings": parameters,
            "timed_wall_time_ns": row["timed_wall_time_ns"],
            "evaluation_time_ns": row["evaluation_time_ns"],
            **statistics,
        }
        measured["envelope_minus_worker_execute_ns"] = measured["timed_wall_time_ns"] - statistics["worker_execute_time_ns"]
        records.append(measured)
        groups[(measured["timing_scope"], parameters["work_ns"], parameters["payload_bytes"], row["compatibility_id"])].append(measured)
    summaries = []
    for (scope, work, payload, compatibility), points in sorted(groups.items()):
        quantities = ["timed_wall_time_ns", "work_time_ns", "result_encoding_time_ns", "result_write_time_ns", "worker_execute_time_ns", "envelope_minus_worker_execute_ns", "evaluation_time_ns"]
        summaries.append({
            "timing_scope": scope, "work_ns": work, "payload_bytes": payload,
            "compatibility_id": compatibility, "repetitions": len(points),
            "median_ns": {name: median(point[name] for point in points) for name in quantities},
            "wall_range_ns": [min(p["timed_wall_time_ns"] for p in points), max(p["timed_wall_time_ns"] for p in points)],
        })
    observation = next(row for row in rows if row["record_type"] == "observation")
    context = observation["provenance"]["data"]["context"]
    return {
        "schema_version": 1,
        "runner": context["runner"],
        "worker": context["implementation"]["executable"],
        "host_observation": context["host_observation"],
        "summaries": summaries,
        "repetitions": records,
        "failed_attempts": [row for row in rows if row["record_type"] == "attempt" and row["outcome"] != "accepted"],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plan", required=True, type=Path)
    parser.add_argument("--jsonl", required=True, type=Path)
    parser.add_argument("--destination", required=True, type=Path)
    parser.add_argument("--command-timing", type=Path, help="Optional captured command list with elapsed_wall_time_ns.")
    args = parser.parse_args()
    report = analyze(json.loads(args.plan.read_text()), [json.loads(line) for line in args.jsonl.read_text().splitlines()])
    report["table_sha256"] = hashlib.sha256(args.jsonl.read_bytes()).hexdigest()
    report["command_timing"] = json.loads(args.command_timing.read_text()) if args.command_timing else None
    with args.destination.open("x") as stream:
        json.dump(report, stream, sort_keys=True, indent=2)
        stream.write("\n")
    print("scope,work_ns,payload_bytes,repetitions,wall_ns,encoding_ns,write_ns,envelope_minus_worker_ns")
    for group in report["summaries"]:
        times = group["median_ns"]
        print(",".join(str(value) for value in [group["timing_scope"], group["work_ns"], group["payload_bytes"], group["repetitions"], times["timed_wall_time_ns"], times["result_encoding_time_ns"], times["result_write_time_ns"], times["envelope_minus_worker_execute_ns"]]))


if __name__ == "__main__":
    main()
