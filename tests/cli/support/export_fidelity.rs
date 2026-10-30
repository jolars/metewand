use std::{collections::BTreeMap, fs, path::Path, process::Command};

use serde_json::Value;

pub fn verify(root: &Path, rows: &[Value]) {
    let mut documents = BTreeMap::new();
    observations(&root.join("run"), &mut documents);
    let output = Command::new("python3")
        .args(["-c", "import csv,json,sys; csv.field_size_limit(sys.maxsize); print(json.dumps(list(csv.DictReader(open(sys.argv[1],newline='')))))"])
        .arg(root.join("csv/results.csv")).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let csv: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(rows.len(), csv.len());
    for (row, csv) in rows.iter().zip(&csv) {
        for field in [
            "profile",
            "timing",
            "provenance",
            "compatibility",
            "evidence",
            "metric_value",
            "provisional_metric_value",
        ] {
            let cell = csv[field].as_str().unwrap();
            let value = if cell.is_empty() {
                Value::Null
            } else {
                serde_json::from_str::<Value>(cell).unwrap()
            };
            assert_eq!(row[field], value, "{field}");
        }
        if row["record_type"] != "observation" {
            continue;
        }
        assert_ne!(row["logical_run_id"], row["resolved_run_id"]);
        assert!(row["logical_run_id"].is_string());
        for field in [
            "logical_run_id",
            "resolved_run_id",
            "logical_slot_id",
            "resolved_slot_id",
            "attempt_id",
        ] {
            assert_eq!(row[field], csv[field]);
        }
        let document = &documents[row["id"].as_str().unwrap()];
        assert_eq!(row["timing"], document["timing"]);
        assert_eq!(row["provenance"], document["provenance"]);
        for (field, value) in document["profile"].as_object().unwrap() {
            assert_eq!(row["profile"][field], *value, "{field}");
        }
        let metrics = &row["evidence"]["metrics"]["document"]["data"];
        assert_eq!(
            row["metric_value"],
            *metrics
                .pointer(row["metric_name"].as_str().unwrap())
                .unwrap()
        );
        assert_eq!(row["compatibility"]["environment_class"], "development");
        assert_eq!(row["compatibility"]["control_class"], "unsupported");
        assert_eq!(row["compatibility"]["timing_scope"], "prepare_and_execute");
        assert_eq!(row["compatibility"]["primary_time"], "timed_wall_time");
    }
}

fn observations(root: &Path, documents: &mut BTreeMap<String, Value>) {
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            observations(&entry.path(), documents);
        } else if entry.file_name() == "observation.json" {
            let document: Value = serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap();
            documents.insert(document["id"].as_str().unwrap().to_owned(), document);
        }
    }
}
