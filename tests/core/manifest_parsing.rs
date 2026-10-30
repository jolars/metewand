use std::path::Path;

use crate::core::manifest::{
    DatasetDefinition, EnvironmentDefinition, NixOutputKind, ParameterAxis, Runner, TimingScope,
    parse_manifest,
};
use serde_json::json;

const COMPLETE_MANIFEST: &str = include_str!("../../fixtures/manifest/v1/complete.yaml");

#[test]
fn concurrency_is_serial_by_default_and_requires_a_positive_safe_integer() {
    let source =
        "version: 1\nname: \"concurrency\"\nexecution-policies:\n  local:\n    version: 1\n";
    let manifest = parse_manifest(Path::new("metewand.yaml"), source).unwrap();
    assert_eq!(
        manifest.execution_policies["local"].resolved_max_concurrency(),
        1
    );
    for value in ["1", "2", "9007199254740991"] {
        let source = format!("{source}    max-concurrency: {value}\n");
        let manifest = parse_manifest(Path::new("metewand.yaml"), &source).unwrap();
        assert_eq!(
            manifest.execution_policies["local"].resolved_max_concurrency(),
            value.parse::<u64>().unwrap()
        );
    }
    for value in ["0", "-1", "1.5", "9007199254740992", "\"2\""] {
        let source = format!("{source}    max-concurrency: {value}\n");
        let error = parse_manifest(Path::new("metewand.yaml"), &source).unwrap_err();
        assert!(error.source_span().is_some(), "{value}");
    }
}

#[test]
fn parses_every_version_one_definition_into_domain_types() {
    let manifest = parse_manifest(Path::new("metewand.yaml"), COMPLETE_MANIFEST).unwrap();

    assert_eq!(manifest.version, 1);
    assert_eq!(manifest.name.as_str(), "complete-benchmark");
    assert_eq!(manifest.datasets.len(), 3);
    assert_eq!(manifest.problems.len(), 1);
    assert_eq!(manifest.implementations.len(), 2);
    assert_eq!(manifest.environments.len(), 5);
    assert_eq!(manifest.experiments.len(), 1);
    assert_eq!(manifest.execution_policies.len(), 1);
    assert_eq!(manifest.observation_policies.len(), 1);

    let generated = &manifest.datasets["generated"];
    let DatasetDefinition::Generated(generated) = generated else {
        panic!("expected a generated dataset");
    };
    assert_eq!(
        generated.parameter_defaults.as_ref().unwrap().as_json(),
        &json!({"rows": 100})
    );
    assert_eq!(generated.worker.runner(), Runner::Python);

    let command = &manifest.datasets["command"];
    let DatasetDefinition::Generated(command) = command else {
        panic!("expected a generated dataset");
    };
    assert_eq!(command.worker.runner(), Runner::Command);
    assert_eq!(
        command.source.as_ref().unwrap().sha256.as_str(),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );

    assert!(matches!(
        manifest.environments["local"],
        EnvironmentDefinition::Local {}
    ));
    assert!(matches!(
        manifest.environments["native"],
        EnvironmentDefinition::Nix {
            output_kind: NixOutputKind::App,
            ..
        }
    ));

    let experiment = &manifest.experiments[0];
    assert!(matches!(
        experiment.problem_parameters["centered"],
        ParameterAxis::Value(_)
    ));
    assert!(matches!(
        experiment.dataset_parameters["generated"]["rows"],
        ParameterAxis::Grid(_)
    ));
    assert_eq!(experiment.cases.len(), 1);

    let policy = &manifest.execution_policies["controlled"];
    assert_eq!(policy.version, 1);
    assert_eq!(policy.timing_scope, Some(TimingScope::PrepareAndExecute));
    assert_eq!(policy.max_concurrency, Some(2));
    assert_eq!(manifest.observation_policies["final"].version, 1);
}

#[test]
fn rejects_unknown_fields_at_every_definition_depth_with_a_source_span() {
    let source = r#"
version: 1
name: "bad-benchmark"
problems:
  example:
    contract: "problems/example.yaml"
    unexpected: true
    evaluator:
      runner: "python"
      entrypoint: "problems/evaluate.py"
      sources: ["problems/evaluate.py"]
      environment: "python"
"#;

    let error = parse_manifest(Path::new("bench/metewand.yaml"), source).unwrap_err();
    assert!(error.message().contains("unknown field `unexpected`"));

    let span = error.source_span().expect("YAML errors must retain a span");
    assert_eq!(span.path, Path::new("bench/metewand.yaml"));
    assert_eq!(span.line, 7);
    assert_eq!(span.column, 5);
    assert_eq!(span.end_line, 7);
    assert!(span.end_column > span.column);
}

#[test]
fn rejects_unknown_fields_across_all_owned_manifest_objects() {
    let fragments = [
        "unexpected: true\n",
        r#"
datasets:
  fixed:
    output-schema: "schemas/dataset.json"
    sources: ["datasets/fixed"]
    unexpected: true
"#,
        r#"
datasets:
  generated:
    output-schema: "schemas/dataset.json"
    runner: "command"
    program: "bin/generate"
    environment: "local"
    source:
      url: "https://example.org/data"
      sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      unexpected: true
"#,
        r#"
problems:
  example:
    contract: "problems/example.yaml"
    evaluator:
      runner: "command"
      program: "bin/evaluate"
      environment: "local"
      unexpected: true
"#,
        r#"
implementations:
  example:
    runner: "command"
    program: "bin/solve"
    environment: "local"
    problem-contracts: ["example"]
    capabilities: ["one_shot"]
    unexpected: true
"#,
        r#"
environments:
  local:
    kind: "local"
    unexpected: true
"#,
        r#"
experiments:
  - name: "main"
    problem: "example"
    implementations: ["example"]
    execution-policy: "default"
    observation-policy: "final"
    implementation-repetitions: 1
    measurement-repetitions: 1
    seed: 0
    unexpected: true
"#,
        r#"
experiments:
  - name: "main"
    problem: "example"
    implementations: ["example"]
    execution-policy: "default"
    observation-policy: "final"
    implementation-repetitions: 1
    measurement-repetitions: 1
    seed: 0
    cases:
      - name: "case"
        unexpected: true
"#,
        r#"
execution-policies:
  default:
    version: 1
    unexpected: true
"#,
        r#"
observation-policies:
  final:
    version: 1
    kind: "one_shot"
    unexpected: true
"#,
        r#"
experiments:
  - name: "main"
    problem: "example"
    implementations: ["example"]
    execution-policy: "default"
    observation-policy: "final"
    implementation-repetitions: 1
    measurement-repetitions: 1
    seed: 0
    problem-parameters:
      alpha: { value: 1, unexpected: true }
"#,
    ];

    for fragment in fragments {
        let source = format!("version: 1\nname: \"strict\"\n{fragment}");
        let error = parse_manifest(Path::new("metewand.yaml"), &source)
            .expect_err("unknown Metewand-owned fields must be rejected");
        assert!(error.source_span().is_some());
    }
}

#[test]
fn requires_an_evaluator_for_every_problem_definition() {
    let source = r#"
version: 1
name: "missing-evaluator"
problems:
  example:
    contract: "problems/example.yaml"
"#;

    let error = parse_manifest(Path::new("metewand.yaml"), source).unwrap_err();
    assert!(error.message().contains("missing field `evaluator`"));
    assert!(error.source_span().is_some());
}

#[test]
fn reports_syntax_errors_against_the_supplied_source_path() {
    let error =
        parse_manifest(Path::new("fixtures/broken.yaml"), "version: 1\nname: [\n").unwrap_err();

    assert!(!error.message().is_empty());
    let span = error
        .source_span()
        .expect("syntax errors must retain a span");
    assert_eq!(span.path, Path::new("fixtures/broken.yaml"));
    assert_eq!(span.line, 2);
}

#[test]
fn retains_only_normalized_repository_relative_paths() {
    let source = r#"
version: 1
name: "normalized-paths"
datasets:
  fixed:
    output-schema: "schemas/dataset.json"
    sources: ["datasets/**/*.json"]
"#;
    let manifest = parse_manifest(Path::new("metewand.yaml"), source).unwrap();
    let DatasetDefinition::Fixed(dataset) = &manifest.datasets["fixed"] else {
        panic!("expected a fixed dataset");
    };
    assert_eq!(
        dataset.sources[0].as_path(),
        Path::new("datasets/**/*.json")
    );

    for path in [
        "/datasets/data.json",
        "datasets//data.json",
        "datasets/./data.json",
        "datasets/../data.json",
        "datasets\\data.json",
        "datasets/data.json/",
    ] {
        let source = format!(
            "version: 1\nname: \"invalid-path\"\ndatasets:\n  fixed:\n    output-schema: \"schemas/dataset.json\"\n    sources: [{path:?}]\n"
        );
        let error = parse_manifest(Path::new("metewand.yaml"), &source).unwrap_err();
        assert!(
            error
                .message()
                .contains("normalized, relative repository path"),
            "unexpected error for {path:?}: {error}"
        );
    }
}

#[test]
fn rejects_unsupported_manifest_and_policy_versions() {
    for source in [
        "version: 2\nname: \"future\"\n",
        r#"
version: 1
name: "future"
execution-policies:
  default:
    version: 2
"#,
        r#"
version: 1
name: "future"
observation-policies:
  default:
    version: 2
    kind: "one_shot"
"#,
    ] {
        let error = parse_manifest(Path::new("metewand.yaml"), source).unwrap_err();
        assert!(error.message().contains("version 1"), "{error}");
        assert!(error.source_span().is_some());
    }
}

#[test]
fn rejects_yaml_values_outside_the_canonical_parameter_domain() {
    let source = r#"
version: 1
name: "bad-parameters"
problems:
  example:
    contract: "problems/example.yaml"
    parameter-defaults: { created: .nan }
    evaluator:
      runner: "command"
      program: "bin/evaluate"
      environment: "local"
"#;

    let error = parse_manifest(Path::new("metewand.yaml"), source).unwrap_err();
    assert!(error.message().contains("non-finite"));
    assert!(error.source_span().is_some());
}

#[test]
fn preserves_open_parameter_names_without_treating_them_as_definition_names() {
    let source = r#"
version: 1
name: "open-parameters"
experiments:
  - name: "main"
    problem: "example"
    implementations: ["example"]
    execution-policy: "default"
    observation-policy: "final"
    implementation-repetitions: 1
    measurement-repetitions: 1
    seed: 0
    problem-parameters:
      "parameter with spaces": { value: 1 }
"#;

    let manifest = parse_manifest(Path::new("metewand.yaml"), source).unwrap();
    assert!(
        manifest.experiments[0]
            .problem_parameters
            .contains_key("parameter with spaces")
    );
}

#[test]
fn rejects_fields_that_are_present_but_invalid_for_the_selected_variant() {
    let source = r#"
version: 1
name: "fixed-with-worker-field"
datasets:
  fixed:
    output-schema: "schemas/dataset.json"
    sources: ["datasets/fixed"]
    args: []
"#;

    let error = parse_manifest(Path::new("metewand.yaml"), source).unwrap_err();
    assert!(error.message().contains("fixed dataset"));
    assert!(error.source_span().is_some());
}
