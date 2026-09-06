use std::{collections::BTreeMap, path::Path};

use metewand_core::{
    manifest::{Manifest, Name, parse_manifest},
    parameters::ParameterResolutionError,
    planning::{
        ConfigurationExpansionError, ConfigurationNamespace, ConfigurationSource,
        DatasetConfigurationDefinition, expand_manifest_configurations,
    },
    problem_contract::{ProblemContract, parse_problem_contract},
    schema::SchemaCatalog,
};
use serde_json::{Value, json};

const CONTRACT: &str = include_str!("../../../fixtures/problem-contract/v1/complete.toml");

const MATRIX_MANIFEST: &str = r#"
version = 1
name = "matrix"

[datasets.generated]
parameter_schema = "schemas/dataset-parameters.json"
parameter_defaults = { nested = { inherited = true }, untouched = "default" }
output_schema = "schemas/dataset.json"
runner = "command"
program = "generate"
environment = "local"

[datasets.fixed]
output_schema = "schemas/dataset.json"
sources = ["datasets/fixed.json"]

[problems.regression]
contract = "problems/regression.toml"
parameter_defaults = { nested = { inherited = true } }
[problems.regression.evaluator]
runner = "command"
program = "evaluate"
environment = "local"

[implementations.tuned]
runner = "command"
program = "solve-tuned"
environment = "local"
problem_contracts = ["regression"]
parameter_schema = "schemas/implementation-parameters.json"
parameter_defaults = { trace = false }
capabilities = ["one_shot"]

[implementations.plain]
runner = "command"
program = "solve-plain"
environment = "local"
problem_contracts = ["regression"]
capabilities = ["one_shot"]

[environments.local]
kind = "local"

[[experiments]]
name = "main"
problem = "regression"
datasets = ["generated", "fixed"]
implementations = ["tuned", "plain"]
execution_policy = "default"
observation_policy = "final"
implementation_repetitions = 1
measurement_repetitions = 1
seed = 17

[experiments.dataset_parameters.generated]
payload = { value = [1, 2] }
rows = { grid = [10, 20] }
nested = { value = { supplied = "dataset" } }

[experiments.problem_parameters]
lambda = { grid = [0.1, 0.2] }
nested = { value = { supplied = "problem" } }

[experiments.implementation_parameters.tuned]
method = { grid = ["a", "b"] }

[execution_policies.default]
version = 1

[observation_policies.final]
version = 1
kind = "one_shot"
"#;

fn schema_catalog() -> SchemaCatalog {
    let open_object_schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object"
    });
    schema_catalog_with_parameter_schemas(open_object_schema.clone(), open_object_schema)
}

fn schema_catalog_with_parameter_schemas(
    dataset_parameters: Value,
    implementation_parameters: Value,
) -> SchemaCatalog {
    let mut documents = [
        (
            "parameters",
            include_str!("../../../fixtures/problem-contract/v1/schemas/parameters.json"),
        ),
        (
            "dataset",
            include_str!("../../../fixtures/problem-contract/v1/schemas/dataset.json"),
        ),
        (
            "result",
            include_str!("../../../fixtures/problem-contract/v1/schemas/result.json"),
        ),
        (
            "metrics",
            include_str!("../../../fixtures/problem-contract/v1/schemas/metrics.json"),
        ),
        (
            "semantics",
            include_str!("../../../fixtures/problem-contract/v1/schemas/semantics.json"),
        ),
    ]
    .into_iter()
    .map(|(name, source)| {
        (
            Path::new("schemas").join(format!("{name}.json")),
            serde_json::from_str::<Value>(source).unwrap(),
        )
    })
    .collect::<Vec<_>>();
    documents.push(("schemas/dataset-parameters.json".into(), dataset_parameters));
    documents.push((
        "schemas/implementation-parameters.json".into(),
        implementation_parameters,
    ));
    SchemaCatalog::try_new(documents).unwrap()
}

fn manifest(source: &str) -> Manifest {
    parse_manifest(Path::new("metewand.toml"), source).unwrap()
}

fn contracts(manifest: &Manifest, dataset_free: bool) -> BTreeMap<Name, ProblemContract> {
    let source = if dataset_free {
        CONTRACT
            .replace(
                "dataset_schemas = [\"schemas/dataset.json\"]",
                "dataset_schemas = []",
            )
            .replace("dataset = \"fixtures/small/dataset\"\n", "")
    } else {
        CONTRACT.to_owned()
    };
    let contract = parse_problem_contract(
        Path::new("problems/regression.toml"),
        &source,
        &schema_catalog(),
    )
    .unwrap();
    BTreeMap::from([(manifest.problems.keys().next().unwrap().clone(), contract)])
}

fn parameters(value: &metewand_core::canonical::CanonicalValue) -> &Value {
    value.as_json()
}

#[test]
fn expands_only_applicable_namespaces_in_a_stable_cartesian_order() {
    let manifest = manifest(MATRIX_MANIFEST);
    let expansion =
        expand_manifest_configurations(&manifest, &contracts(&manifest, false), &schema_catalog())
            .unwrap();

    assert_eq!(expansion.len(), 1);
    let candidates = &expansion[0].candidates;
    assert_eq!(candidates.len(), 18);

    let summary = candidates
        .iter()
        .map(|candidate| {
            let DatasetConfigurationDefinition::Named(dataset) = &candidate.dataset.definition
            else {
                panic!("the matrix uses only named datasets");
            };
            (
                dataset.as_str(),
                parameters(&candidate.dataset.parameters).clone(),
                parameters(&candidate.problem.parameters).clone(),
                candidate.implementation.definition.as_str(),
                parameters(&candidate.implementation.parameters).clone(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        summary[0],
        (
            "generated",
            json!({
                "nested": {"inherited": true, "supplied": "dataset"},
                "payload": [1, 2],
                "rows": 10,
                "untouched": "default"
            }),
            json!({
                "lambda": 0.1,
                "nested": {"inherited": true, "supplied": "problem"}
            }),
            "tuned",
            json!({"method": "a", "trace": false}),
        )
    );
    assert_eq!(summary[1].4, json!({"method": "b", "trace": false}));
    assert_eq!(summary[2].3, "plain");
    assert_eq!(summary[2].4, json!({}));
    assert_eq!(summary[3].2["lambda"], json!(0.2));
    assert_eq!(summary[6].1["rows"], json!(20));
    assert_eq!(summary[12].0, "fixed");
    assert_eq!(summary[12].1, json!({}));
}

#[test]
fn named_cases_are_unioned_and_keep_coupled_values_together() {
    let source = MATRIX_MANIFEST.replace(
        "[execution_policies.default]",
        r#"
[[experiments.cases]]
name = "tall_dense"
[experiments.cases.dataset_parameters.generated]
rows = { value = 10000 }
density = { value = 1.0 }
[experiments.cases.problem_parameters]
lambda = { value = 0.01 }
[experiments.cases.implementation_parameters.tuned]
method = { value = "dense" }

[[experiments.cases]]
name = "wide_sparse"
[experiments.cases.dataset_parameters.generated]
rows = { value = 1000 }
density = { value = 0.01 }
[experiments.cases.problem_parameters]
lambda = { value = 0.1 }
[experiments.cases.implementation_parameters.tuned]
method = { value = "sparse" }

[execution_policies.default]"#,
    );
    let manifest = manifest(&source);
    let expansion =
        expand_manifest_configurations(&manifest, &contracts(&manifest, false), &schema_catalog())
            .unwrap();
    let candidates = &expansion[0].candidates;

    assert_eq!(candidates.len(), 8);
    assert!(
        candidates[..4]
            .iter()
            .all(|candidate| candidate.source == ConfigurationSource::Case(name("tall_dense")))
    );
    assert!(
        candidates[4..]
            .iter()
            .all(|candidate| candidate.source == ConfigurationSource::Case(name("wide_sparse")))
    );
    let generated = candidates
        .iter()
        .filter(|candidate| {
            candidate.dataset.definition == DatasetConfigurationDefinition::Named(name("generated"))
        })
        .collect::<Vec<_>>();
    assert!(generated[..2].iter().all(|candidate| {
        candidate.dataset.parameters.as_json()["rows"] == json!(10000)
            && candidate.dataset.parameters.as_json()["density"] == json!(1)
            && candidate.problem.parameters.as_json()["lambda"] == json!(0.01)
    }));
    assert!(generated[2..].iter().all(|candidate| {
        candidate.dataset.parameters.as_json()["rows"] == json!(1000)
            && candidate.dataset.parameters.as_json()["density"] == json!(0.01)
            && candidate.problem.parameters.as_json()["lambda"] == json!(0.1)
    }));

    let tuned = candidates
        .iter()
        .filter(|candidate| candidate.implementation.definition.as_str() == "tuned")
        .collect::<Vec<_>>();
    assert_eq!(tuned.len(), 4);
    assert_eq!(
        tuned[0].implementation.parameters.as_json()["method"],
        "dense"
    );
    assert_eq!(
        tuned[2].implementation.parameters.as_json()["method"],
        "sparse"
    );
}

#[test]
fn rejects_duplicate_candidates_from_overlapping_cases_after_resolution() {
    let source = MATRIX_MANIFEST
        .replace("rows = { grid = [10, 20] }", "rows = { value = 10 }")
        .replace("lambda = { grid = [0.1, 0.2] }", "lambda = { value = 0.1 }")
        .replace(
            "method = { grid = [\"a\", \"b\"] }",
            "method = { value = \"a\" }",
        )
        .replace(
            "[execution_policies.default]",
            r#"
[[experiments.cases]]
name = "first"
[experiments.cases.dataset_parameters.generated]
rows = { value = 10 }

[[experiments.cases]]
name = "overlap"
[experiments.cases.dataset_parameters.generated]
rows = { grid = [10, 11] }

[execution_policies.default]"#,
        );
    let manifest = manifest(&source);
    let error =
        expand_manifest_configurations(&manifest, &contracts(&manifest, false), &schema_catalog())
            .unwrap_err();

    let ConfigurationExpansionError::DuplicateLogicalCandidate(error) = error else {
        panic!("expected a duplicate logical candidate error");
    };
    assert!(matches!(
        *error,
        metewand_core::planning::DuplicateLogicalCandidateError {
            experiment,
            first_source: ConfigurationSource::Case(first),
            duplicate_source: ConfigurationSource::Case(duplicate),
            dataset: DatasetConfigurationDefinition::Named(dataset),
            problem,
            implementation,
        } if experiment.as_str() == "main"
            && first.as_str() == "first"
            && duplicate.as_str() == "overlap"
            && dataset.as_str() == "generated"
            && problem.as_str() == "regression"
            && implementation.as_str() == "tuned"
    ));
}

#[test]
fn rejects_repeated_grid_members_as_duplicate_candidates() {
    let source = MATRIX_MANIFEST
        .replace("rows = { grid = [10, 20] }", "rows = { grid = [10, 10] }")
        .replace("lambda = { grid = [0.1, 0.2] }", "lambda = { value = 0.1 }")
        .replace(
            "method = { grid = [\"a\", \"b\"] }",
            "method = { value = \"a\" }",
        );
    let manifest = manifest(&source);

    assert!(matches!(
        expand_manifest_configurations(&manifest, &contracts(&manifest, false), &schema_catalog(),),
        Err(ConfigurationExpansionError::DuplicateLogicalCandidate(error))
            if error.first_source == ConfigurationSource::Experiment
                && error.duplicate_source == ConfigurationSource::Experiment
    ));
}

#[test]
fn binds_one_explicit_unit_dataset_for_dataset_free_problems() {
    let source = MATRIX_MANIFEST
        .replace("datasets = [\"generated\", \"fixed\"]\n", "")
        .replace(
            "[experiments.dataset_parameters.generated]\npayload = { value = [1, 2] }\nrows = { grid = [10, 20] }\nnested = { value = { supplied = \"dataset\" } }\n\n",
            "",
        );
    let manifest = manifest(&source);
    let expansion =
        expand_manifest_configurations(&manifest, &contracts(&manifest, true), &schema_catalog())
            .unwrap();

    assert_eq!(expansion[0].candidates.len(), 6);
    assert!(expansion[0].candidates.iter().all(|candidate| {
        candidate.dataset.definition == DatasetConfigurationDefinition::Unit
            && candidate.dataset.parameters.as_json() == &json!({})
    }));
}

#[test]
fn validates_every_resolved_configuration_with_its_own_schema() {
    let source = MATRIX_MANIFEST.replace("rows = { grid = [10, 20] }", "rows = { grid = [10, 0] }");
    let manifest = manifest(&source);
    let schemas = schema_catalog_with_parameter_schemas(
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {"rows": {"type": "integer", "minimum": 1}}
        }),
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object"
        }),
    );

    assert!(matches!(
        expand_manifest_configurations(&manifest, &contracts(&manifest, false), &schemas),
        Err(ConfigurationExpansionError::ParameterResolution {
            experiment,
            source_location: ConfigurationSource::Experiment,
            namespace: ConfigurationNamespace::Dataset,
            definition,
            error: ParameterResolutionError::Validation(_),
        }) if experiment.as_str() == "main" && definition.as_str() == "generated"
    ));
}

#[test]
fn parameter_table_order_does_not_change_expansion_order() {
    let reordered = MATRIX_MANIFEST
        .replace(
            "payload = { value = [1, 2] }\nrows = { grid = [10, 20] }\nnested = { value = { supplied = \"dataset\" } }",
            "nested = { value = { supplied = \"dataset\" } }\nrows = { grid = [10, 20] }\npayload = { value = [1, 2] }",
        )
        .replace(
            "lambda = { grid = [0.1, 0.2] }\nnested = { value = { supplied = \"problem\" } }",
            "nested = { value = { supplied = \"problem\" } }\nlambda = { grid = [0.1, 0.2] }",
        );
    let left = manifest(MATRIX_MANIFEST);
    let right = manifest(&reordered);

    assert_eq!(
        expand_manifest_configurations(&left, &contracts(&left, false), &schema_catalog()).unwrap(),
        expand_manifest_configurations(&right, &contracts(&right, false), &schema_catalog())
            .unwrap()
    );
}

fn name(value: &str) -> Name {
    manifest(&format!("version = 1\nname = {value:?}\n")).name
}
