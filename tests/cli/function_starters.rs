#![cfg(unix)]

use std::{
    env,
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use crate::core::public_schemas::{PublicSchema, public_schema_catalog};
use serde_json::Value;
use tempfile::TempDir;

struct Starter {
    _root: TempDir,
    project: PathBuf,
    path: OsString,
}

#[test]
fn name_filters_select_workers_and_resume_with_stable_slots() {
    let starter = Starter::new("python");
    fs::write(
        starter.project.join("benchmark.py"),
        r#"
def generate(context): return {"values": [1, 2]}
def solve(data, context): return {"total": sum(data["values"])}
def evaluate(data, result, context): return {"error": abs(result["total"] - sum(data["values"]))}
"#,
    )
    .unwrap();
    fs::write(
        starter.project.join("unavailable.py"),
        "raise RuntimeError('unselected worker imported')\n",
    )
    .unwrap();
    fs::write(
        starter.project.join("metewand.yaml"),
        r#"
version: 1
name: filters
default-experiment: full
datasets:
  first:
    function: benchmark.py:generate
    parameters: {size: {type: integer, default: 2}}
  second:
    function: benchmark.py:generate
    parameters: {size: {type: integer, default: 3}}
  unavailable: {function: unavailable.py:generate}
problems:
  sum:
    result: {total: number}
    metrics: {error: number}
    evaluator: {function: benchmark.py:evaluate}
  other:
    result: {total: number}
    metrics: {error: number}
    evaluator: {function: unavailable.py:evaluate}
implementations:
  alpha:
    function: benchmark.py:solve
    problem-contracts: [sum, other]
    parameters: {steps: {type: integer, default: 1}}
  beta:
    function: benchmark.py:solve
    problem-contracts: [sum, other]
    parameters: {steps: {type: integer, default: 2}}
  unavailable: {function: unavailable.py:solve, problem-contracts: [sum, other]}
experiments:
  - {name: full, problem: sum, datasets: [first, second, unavailable], implementations: [alpha, beta, unavailable], seed: 7}
  - {name: other, problem: other, datasets: [first], implementations: [alpha], seed: 7}
"#,
    )
    .unwrap();
    let baseline = starter.json(&["plan"], 0);
    let expected = baseline["experiments"][0]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| {
            candidate["parameters"]["dataset"]["size"] == 3
                && candidate["parameters"]["implementation"]["steps"] == 1
        })
        .unwrap();
    let filters = [
        "--problem",
        "sum",
        "--implementation",
        "alpha",
        "--dataset",
        "second",
    ];
    let args = |command| std::iter::once(command).chain(filters).collect::<Vec<_>>();
    let selected = starter.json(&args("plan"), 0);
    assert_eq!(selected["summary"]["logical_candidates"], 1);
    assert_eq!(selected["experiments"][0]["candidates"][0], *expected);
    assert_eq!(
        starter.json(
            &[
                "plan",
                "--implementation",
                "alpha",
                "--verbose",
                "--manifest",
                "metewand.yaml",
                "--dataset",
                "second",
                "--problem",
                "sum",
            ],
            0,
        ),
        selected
    );
    let human = starter.invoke(&args("plan"));
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(human.contains("second"), "{human}");
    assert!(!human.contains("unavailable"), "{human}");
    let mut check = args("check");
    check.push("--workers");
    let checked = starter.json(&check, 0);
    assert_eq!(checked["worker_launches"], 3);
    assert_eq!(checked["workers"].as_array().unwrap().len(), 3);
    let excluded = &baseline["experiments"][0]["candidates"][0];
    let mut invalid = args("run");
    invalid.extend([
        "--select",
        excluded["id"].as_str().unwrap(),
        "--output-dir",
        "excluded",
    ]);
    let diagnostic = starter.json(&invalid, 2);
    assert!(
        diagnostic["message"]
            .as_str()
            .unwrap()
            .contains("matches no measured slot")
    );
    assert!(!starter.project.join("excluded").exists());
    let mut run = args("run");
    run.extend(["--select", expected["id"].as_str().unwrap()]);
    let first = starter.json(&run, 0);
    assert_eq!(first["summary"]["accepted_attempts"], 1);
    assert_eq!(
        first["attempts"][0]["logical_slot_id"],
        expected["specifications"][0]["attempt_slots"][0]
    );
    let repeated = starter.json(&run, 0);
    assert_eq!(repeated["summary"]["attempt_slots"], 0);
    let mut recorded = args("run");
    recorded.extend([
        "--select",
        first["attempts"][0]["attempt_id"].as_str().unwrap(),
    ]);
    let repeated = starter.json(&recorded, 0);
    assert_eq!(repeated["summary"]["attempt_slots"], 0);
    let broader = starter.json(
        &[
            "run",
            "full",
            "other",
            "--problem",
            "sum",
            "--dataset",
            "second",
            "--dataset",
            "first",
            "--dataset",
            "first",
            "--implementation",
            "beta",
            "--implementation",
            "alpha",
        ],
        0,
    );
    assert_eq!(broader["summary"]["accepted_attempts"], 3);
    assert!(
        broader["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|attempt| {
                attempt["logical_slot_id"] != first["attempts"][0]["logical_slot_id"]
            })
    );
}

#[test]
fn named_experiments_isolate_workers_and_record_explicit_environment() {
    let starter = Starter::new("python");
    fs::write(starter.project.join("benchmark.py"), r#"
import os
def generate(context): return {"values": [1, 2]}
def solve(data, context):
    assert os.environ["OMP_NUM_THREADS"] == "1"
    return {"total": sum(data["values"])}
def evaluate(data, result, context): return {"error": abs(result["total"] - sum(data["values"])) + 1e-13}
"#).unwrap();
    fs::write(
        starter.project.join("unavailable.py"),
        "raise RuntimeError('unselected worker imported')\n",
    )
    .unwrap();
    fs::write(
        starter.project.join("metewand.yaml"),
        r#"
version: 1
name: selection
default-experiment: smoke
environments:
  local:
    kind: local
    environment-variables: {OMP_NUM_THREADS: "1"}
datasets:
  numbers: {function: benchmark.py:generate}
problems:
  sum:
    result: {total: number}
    metrics: {error: number}
    evaluator: {function: benchmark.py:evaluate}
    reference-cases:
      - dataset: {values: [1, 2]}
        result: {total: 3}
        expected-metrics: {error: 0}
        metric-tolerance: {absolute: 1e-12}
implementations:
  available: {function: benchmark.py:solve}
  unavailable: {function: unavailable.py:solve}
experiments:
  - {name: smoke, problem: sum, datasets: [numbers], implementations: [available], seed: 7}
  - {name: full, problem: sum, datasets: [numbers], implementations: [unavailable], seed: 7}
"#,
    )
    .unwrap();
    let plan = starter.json(&["plan"], 0);
    assert_eq!(plan["summary"]["logical_candidates"], 1);
    let checked = starter.json(&["check", "--workers"], 0);
    assert_eq!(checked["worker_launches"], 3);
    assert_eq!(checked["reference_cases"][0]["metrics"]["error"], 1e-13);
    let selected = starter.json(
        &["plan", "--experiment", "full", "--experiment", "smoke"],
        0,
    );
    assert_eq!(selected["summary"]["logical_candidates"], 2);
    let run = starter.json(&["run", "--experiment", "smoke", "--output-dir", "run"], 0);
    assert_eq!(run["summary"]["accepted_attempts"], 1);
    let positional_check = starter.json(&["check", "smoke", "--workers"], 0);
    assert_eq!(positional_check, checked);
    let resumed = starter.json(&["run", "smoke", "--output-dir", "run", "--resume"], 0);
    assert_eq!(resumed["summary"]["attempt_slots"], 0);
    let path = starter.project.join("metewand.yaml");
    let manifest = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        manifest.replace("default-experiment: smoke", "default-experiment: full"),
    )
    .unwrap();
    let positional_run = starter.json(&["run", "--output-dir", "positional", "smoke"], 0);
    assert_eq!(positional_run["summary"]["accepted_attempts"], 1);
    assert_eq!(
        positional_run["comparison"][0]["metrics"],
        run["comparison"][0]["metrics"]
    );
    let attempts = fs::read_dir(starter.project.join("run/artifacts")).unwrap();
    let receipt = attempts
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("attempt.json"))
        .find(|path| path.exists())
        .unwrap();
    let receipt: Value = serde_json::from_slice(&fs::read(receipt).unwrap()).unwrap();
    assert_eq!(
        receipt["provenance"]["data"]["execution"]["implementation"]["environment_variables"]["OMP_NUM_THREADS"],
        "1"
    );
    let failed = starter.invoke(&["check", "--workers", "--experiment", "full"]);
    assert!(!failed.status.success());
    let unknown = starter.invoke(&["run", "--experiment", "missing", "--output-dir", "missing"]);
    assert!(!unknown.status.success());
    assert!(!starter.project.join("missing").exists());
    for name in ["PATH", "HOME", "METEWAND_PROTOCOL_TRANSPORT"] {
        fs::write(
            &path,
            manifest.replace("OMP_NUM_THREADS: \"1\"", &format!("{name}: \"1\"")),
        )
        .unwrap();
        let output = starter.invoke(&["check", "--output", "json"]);
        assert!(!output.status.success());
        let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            envelope["data"]["causes"][0]["code"],
            "invalid_worker_environment"
        );
    }
}

#[test]
fn r_functions_share_problem_preparation_with_the_evaluator() {
    let starter = Starter::new("r");
    let manifest = include_str!("../../examples/problem-preparation/metewand.yaml")
        .replace("functions.py", "benchmark.R")
        .replace("preparation.py", "preparation.R");
    fs::write(starter.project.join("metewand.yaml"), manifest).unwrap();
    fs::write(starter.project.join("benchmark.R"), r#"
generate <- function(context) list(values = metewand::json_array(seq_len(context$parameters$size) - 1))
solve <- function(data, context) {
  grid <- metewand::read_json(file.path(context$problem_dir, "data.json"))$alpha
  list(total = sum(unlist(data$values)) * sum(unlist(grid)))
}
solve_loop <- solve
evaluate <- function(data, result, context) list(error = abs(result$total - solve(data, context)$total))
"#).unwrap();
    fs::write(
        starter.project.join("preparation.R"),
        r#"
prepare_path <- function(data, context) {
  parameters <- context$problem_parameters
  length <- min(parameters$path_length, length(data$values) - 2)
  scale <- max(unlist(data$values)) + as.integer(parameters$fit_intercept)
  list(alpha = metewand::json_array(scale * parameters$q ^ (seq_len(length) - 1)))
}
"#,
    )
    .unwrap();
    let checked = starter.json(&["check", "--workers"], 0);
    assert!(
        checked["workers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|worker| worker["role"] == "problem_preparer")
    );
    let run = starter.json(&["run", "--output-dir", "run"], 0);
    assert_eq!(run["summary"]["accepted_attempts"], 8);
    for row in run["comparison"].as_array().unwrap() {
        assert_eq!(row["metrics"]["error"], 0);
    }
}

fn copy(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == "__pycache__" {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

impl Starter {
    fn new(language: &str) -> Self {
        let root = TempDir::new().unwrap();
        let project = root.path().join("benchmark");
        let result = Command::new(env!("CARGO_BIN_EXE_metewand"))
            .args(["init", "--language", language])
            .arg(&project)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
        let bin = if language == "python" {
            let venv = root.path().join("venv");
            let result = Command::new("python3")
                .args(["-m", "venv", "--without-pip"])
                .arg(&venv)
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
            let site = fs::read_dir(venv.join("lib"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path()
                .join("site-packages");
            copy(
                &repository.join("packages/python/src/metewand"),
                &site.join("metewand"),
            );
            venv.join("bin")
        } else {
            let library = root.path().join("library");
            fs::create_dir(&library).unwrap();
            let package = root.path().join("sdk");
            copy(&repository.join("packages/r"), &package);
            let result = Command::new("R")
                .args(["CMD", "INSTALL", "--preclean", "--clean"])
                .arg(format!("--library={}", library.display()))
                .arg(package)
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
            let interpreter = env::split_paths(&env::var_os("PATH").unwrap())
                .map(|p| p.join("Rscript"))
                .find(|p| p.is_file())
                .unwrap();
            let bin = root.path().join("bin");
            fs::create_dir(&bin).unwrap();
            let script = bin.join("Rscript");
            // Preserve the site libraries that provide dependencies such as jsonlite.
            fs::write(
                &script,
                format!(
                    "#!/bin/sh\nexport R_LIBS='{}'${{R_LIBS:+:}}${{R_LIBS:-}}\nexec '{}' \"$@\"\n",
                    library.display(),
                    interpreter.display()
                ),
            )
            .unwrap();
            fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
            bin
        };
        let path = env::join_paths(
            std::iter::once(bin).chain(env::split_paths(&env::var_os("PATH").unwrap())),
        )
        .unwrap();
        Self {
            _root: root,
            project,
            path,
        }
    }

    fn invoke(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_metewand"))
            .current_dir(&self.project)
            .env("PATH", &self.path)
            .args(args)
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str], exit: i32) -> Value {
        let mut args = args.to_vec();
        args.extend(["--output", "json"]);
        let output = self.invoke(&args);
        assert_eq!(output.status.code(), Some(exit), "{output:?}");
        let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
        public_schema_catalog()
            .unwrap()
            .validate(PublicSchema::MachineOutput.repository_path(), &envelope)
            .unwrap();
        envelope["data"].clone()
    }
}

#[test]
fn python_and_r_starters_run_inspect_resume_and_export_without_scientific_boilerplate() {
    for language in ["python", "r"] {
        let starter = Starter::new(language);
        let check = starter.json(&["check", "--workers"], 0);
        assert_eq!(check["reference_cases"].as_array().unwrap().len(), 0);
        let plan = starter.json(&["plan"], 0);
        assert_eq!(plan["summary"]["attempt_slots"], 2);
        let run = starter.json(&["run", "--output-dir", "run"], 0);
        assert_eq!(run["summary"]["accepted_attempts"], 2);
        let rows = run["comparison"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        for row in rows {
            assert!(matches!(
                row["implementation"].as_str(),
                Some("builtin" | "loop")
            ));
            assert_eq!(row["metrics"]["absolute_error"], 0);
            assert_eq!(row["completion"], "not_required");
            assert_eq!(row["parameters"]["dataset"]["size"], 20);
            assert!(row["timed_wall_time_ns"].as_u64().unwrap() > 0);
            assert_eq!(row["timing_scope"], "prepare_and_execute");
        }
        let resume = starter.json(&["run", "--output-dir", "run", "--resume"], 0);
        assert!(resume["attempts"].as_array().unwrap().is_empty());
        assert_eq!(resume["comparison"], run["comparison"]);
        let human = starter.invoke(&["status", "--output-dir", "run"]);
        let text = String::from_utf8(human.stdout).unwrap();
        let methods = text
            .lines()
            .filter(|line| line.contains('|'))
            .map(|line| line.split('|').next().unwrap().trim())
            .collect::<Vec<_>>();
        assert_eq!(methods, ["Method", "loop", "builtin"], "{text}");
        fs::remove_file(starter.project.join("metewand.yaml")).unwrap();
        let status = starter.json(&["status", "--output-dir", "run"], 0);
        assert_eq!(status["comparison"], run["comparison"]);
        for format in ["jsonl", "csv"] {
            starter.json(
                &[
                    "export",
                    "--output-dir",
                    "run",
                    "--format",
                    format,
                    "--destination",
                    format,
                ],
                0,
            );
        }
    }
}

#[test]
fn static_checks_never_import_the_source_and_missing_functions_have_diagnostics() {
    let starter = Starter::new("python");
    let file = starter.project.join("benchmark.py");
    let original = fs::read_to_string(&file).unwrap();
    fs::write(
        &file,
        format!("raise RuntimeError('source imported')\n{original}"),
    )
    .unwrap();
    starter.json(&["check"], 0);
    starter.json(&["plan"], 0);
    let error = starter.json(&["check", "--workers"], 3);
    assert!(error.to_string().contains("source imported"), "{error}");
    fs::write(file, original).unwrap();
    let manifest = starter.project.join("metewand.yaml");
    let source = fs::read_to_string(&manifest)
        .unwrap()
        .replace(":builtin_sum", ":missing_function");
    fs::write(manifest, source).unwrap();
    let error = starter.json(&["check", "--workers"], 3);
    assert!(error.to_string().contains("missing_function"), "{error}");
}

#[test]
fn missing_function_sdk_has_an_actionable_check_diagnostic() {
    let starter = Starter::new("python");
    let interpreter = env::split_paths(&starter.path).next().unwrap();
    let library = fs::read_dir(interpreter.parent().unwrap().join("lib"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::remove_dir_all(library.join("site-packages/metewand")).unwrap();
    starter.json(&["check"], 0);
    let output = starter.invoke(&["check", "--workers", "--output", "json"]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let diagnostic: Value = serde_json::from_slice(&output.stdout).unwrap();
    let text = diagnostic.to_string();
    assert!(text.contains("worker_check_failed"), "{text}");
    assert!(
        text.contains("install a compatible Metewand binding package"),
        "{text}"
    );
}

#[test]
fn scientific_rules_and_inline_reference_cases_are_an_incremental_addition() {
    let starter = Starter::new("python");
    let manifest = starter.project.join("metewand.yaml");
    let source = fs::read_to_string(&manifest).unwrap().replace("    evaluator:", "    semantics:\n      one_shot_completion: {absolute_error: 0}\n    reference-cases:\n      - dataset: {values: [1, 2]}\n        result: {total: 3}\n        expected-metrics: {absolute_error: 0}\n      - dataset: {values: [1, 2]}\n        result: {total: 0}\n        expected-metrics: {absolute_error: 3}\n        expected-verdict: not_reached\n    evaluator:");
    fs::write(&manifest, source).unwrap();
    let failed = starter.json(&["run", "--output-dir", "missing-verdict"], 5);
    assert_eq!(failed["summary"]["failed_attempts"], 2);
    let file = starter.project.join("benchmark.py");
    let source = fs::read_to_string(&file).unwrap().replace("    return {\"absolute_error\": abs(result[\"total\"] - sum(data[\"values\"]))}",
        "    from metewand import Evaluation\n    error = abs(result[\"total\"] - sum(data[\"values\"]))\n    target = context.semantics[\"one_shot_completion\"][\"absolute_error\"]\n    return Evaluation({\"absolute_error\": error}, \"reached\" if error <= target else \"not_reached\")");
    fs::write(&file, source).unwrap();
    let checked = starter.json(&["check", "--workers"], 0);
    assert_eq!(checked["reference_cases"].as_array().unwrap().len(), 2);
    let source = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        source.replace("expected-verdict: not_reached", "expected-verdict: reached"),
    )
    .unwrap();
    let mismatch = starter.json(&["check", "--workers"], 3);
    assert!(
        mismatch
            .to_string()
            .contains("not_reached differs from expected reached"),
        "{mismatch}"
    );
    fs::write(&manifest, source).unwrap();
    let run = starter.json(&["run", "--output-dir", "validated"], 0);
    assert!(
        run["comparison"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["completion"] == "reached")
    );
    let source = fs::read_to_string(&file)
        .unwrap()
        .replace("return {\"total\": total}", "return {\"total\": total + 1}");
    fs::write(file, source).unwrap();
    let run = starter.json(&["run", "--output-dir", "missed-target"], 5);
    assert_eq!(run["summary"]["failed_attempts"], 1);
    let failed = run["comparison"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["status"] == "failed")
        .unwrap();
    assert_eq!(
        failed["diagnostic"]["code"],
        "completion_target_not_reached"
    );
    assert_eq!(failed["metrics"]["absolute_error"], 1);
}

#[test]
fn least_squares_example_adds_validation_without_replacing_scientific_functions() {
    let starter = Starter::new("python");
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/least-squares");
    copy(&example, &starter.project);
    let basic = starter.json(&["run", "--output-dir", "basic"], 0);
    assert_eq!(basic["summary"]["accepted_attempts"], 2);
    for row in basic["comparison"].as_array().unwrap() {
        assert_eq!(row["completion"], "not_required");
        assert!(row["metrics"]["gradient_inf"].as_f64().unwrap() < 1e-8);
    }
    let checked = starter.json(&["check", "--manifest", "validated.yaml", "--workers"], 0);
    assert_eq!(checked["reference_cases"].as_array().unwrap().len(), 3);
    let run = starter.json(
        &[
            "run",
            "--manifest",
            "validated.yaml",
            "--output-dir",
            "validated",
        ],
        5,
    );
    assert_eq!(run["summary"]["accepted_attempts"], 2);
    assert_eq!(run["summary"]["failed_attempts"], 2);
    assert_eq!(run["comparison"].as_array().unwrap().len(), 4);
}
