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
        assert!(
            text.contains("Method | Rep") && text.contains("builtin |") && text.contains("loop |"),
            "{text}"
        );
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
