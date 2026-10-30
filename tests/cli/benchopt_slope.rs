#![cfg(unix)]

use std::{env, path::Path, process::Command};

use tempfile::TempDir;

#[test]
#[ignore = "Requires benchmark_slope and Python with its scientific dependencies."]
fn benchopt_slope_comparison_retains_completion_misses() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let upstream = env::var_os("METEWAND_SLOPE_ROOT").expect("Set METEWAND_SLOPE_ROOT.");
    let python = env::var_os("METEWAND_SLOPE_PYTHON").expect("Set METEWAND_SLOPE_PYTHON.");
    let python = Path::new(&python);
    assert!(
        python.is_absolute(),
        "METEWAND_SLOPE_PYTHON must be absolute."
    );
    let path = env::join_paths(
        std::iter::once(python.parent().unwrap().to_owned())
            .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
    )
    .unwrap();
    let temporary = TempDir::new().unwrap();
    let benchmark = temporary.path().join("benchmark");
    let prepared = Command::new(python)
        .arg(root.join("examples/benchopt-slope/prepare.py"))
        .arg("--benchmark")
        .arg(upstream)
        .arg("--destination")
        .arg(&benchmark)
        .output()
        .unwrap();
    assert!(prepared.status.success(), "{prepared:?}");
    let validated = Command::new(python)
        .arg(benchmark.join("validate.py"))
        .arg("--metewand")
        .arg(env!("CARGO_BIN_EXE_metewand"))
        .arg("--output-dir")
        .arg(temporary.path().join("run"))
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(validated.status.success(), "{validated:?}");
}
