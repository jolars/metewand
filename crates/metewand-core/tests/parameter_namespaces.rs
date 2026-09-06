use std::path::Path;

use metewand_core::{
    manifest::parse_manifest,
    parameters::{ParameterNamespace, ParameterNamespaceError, validate_parameter_namespaces},
};

const COMPLETE_MANIFEST: &str = include_str!("../../../fixtures/manifest/v1/complete.toml");

fn parse(source: &str) -> metewand_core::manifest::Manifest {
    parse_manifest(Path::new("metewand.toml"), source).unwrap()
}

#[test]
fn accepts_explicit_namespaces_without_inferring_scientific_ownership() {
    validate_parameter_namespaces(&parse(COMPLETE_MANIFEST)).unwrap();

    let manifest = parse(
        r#"
version = 1
name = "visible-ownership"

[datasets.generated]
parameter_schema = "schemas/dataset-parameters.json"
output_schema = "schemas/dataset.json"
runner = "command"
program = "generate"
environment = "local"

[problems.example]
contract = "problems/example.toml"
[problems.example.evaluator]
runner = "command"
program = "evaluate"
environment = "local"

[implementations.example]
runner = "command"
program = "solve"
environment = "local"
problem_contracts = ["example"]
parameter_schema = "schemas/implementation-parameters.json"
capabilities = ["one_shot"]

[[experiments]]
name = "main"
problem = "example"
datasets = ["generated"]
implementations = ["example"]
execution_policy = "default"
observation_policy = "final"
implementation_repetitions = 1
measurement_repetitions = 1
seed = 0

[experiments.dataset_parameters.generated]
scale = { value = 1 }

[experiments.problem_parameters]
scale = { value = 2 }

[experiments.implementation_parameters.example]
scale = { value = 3 }
"#,
    );

    validate_parameter_namespaces(&manifest).unwrap();
}

#[test]
fn requires_a_schema_for_definition_owned_parameter_defaults() {
    let dataset = parse(
        r#"
version = 1
name = "dataset-defaults"

[datasets.generated]
parameter_defaults = { rows = 100 }
output_schema = "schemas/dataset.json"
runner = "command"
program = "generate"
environment = "local"
"#,
    );
    assert!(matches!(
        validate_parameter_namespaces(&dataset),
        Err(ParameterNamespaceError::DefaultsWithoutSchema {
            namespace: ParameterNamespace::Dataset,
            definition,
        }) if definition.as_str() == "generated"
    ));

    let implementation = parse(
        r#"
version = 1
name = "implementation-defaults"

[implementations.example]
runner = "command"
program = "solve"
environment = "local"
problem_contracts = ["problem"]
parameter_defaults = { tolerance = 1e-8 }
capabilities = ["one_shot"]
"#,
    );
    assert!(matches!(
        validate_parameter_namespaces(&implementation),
        Err(ParameterNamespaceError::DefaultsWithoutSchema {
            namespace: ParameterNamespace::Implementation,
            definition,
        }) if definition.as_str() == "example"
    ));
}

#[test]
fn rejects_parameter_namespaces_for_unselected_definitions() {
    let manifest = parse(
        r#"
version = 1
name = "unselected"

[datasets.selected]
parameter_schema = "schemas/dataset-parameters.json"
output_schema = "schemas/dataset.json"
runner = "command"
program = "generate"
environment = "local"

[datasets.spare]
parameter_schema = "schemas/dataset-parameters.json"
output_schema = "schemas/dataset.json"
runner = "command"
program = "generate"
environment = "local"

[[experiments]]
name = "main"
problem = "problem"
datasets = ["selected"]
implementations = ["implementation"]
execution_policy = "default"
observation_policy = "final"
implementation_repetitions = 1
measurement_repetitions = 1
seed = 0

[experiments.dataset_parameters.spare]
rows = { value = 100 }
"#,
    );

    assert!(matches!(
        validate_parameter_namespaces(&manifest),
        Err(ParameterNamespaceError::UnselectedDefinition {
            location,
            namespace: ParameterNamespace::Dataset,
            definition,
        }) if location.experiment.as_str() == "main"
            && location.case.is_none()
            && definition.as_str() == "spare"
    ));

    let implementation = parse(
        r#"
version = 1
name = "unselected-implementation"

[implementations.selected]
runner = "command"
program = "solve"
environment = "local"
problem_contracts = ["problem"]
parameter_schema = "schemas/implementation-parameters.json"
capabilities = ["one_shot"]

[implementations.spare]
runner = "command"
program = "solve"
environment = "local"
problem_contracts = ["problem"]
parameter_schema = "schemas/implementation-parameters.json"
capabilities = ["one_shot"]

[[experiments]]
name = "main"
problem = "problem"
implementations = ["selected"]
execution_policy = "default"
observation_policy = "final"
implementation_repetitions = 1
measurement_repetitions = 1
seed = 0

[experiments.implementation_parameters.spare]
tolerance = { value = 1e-8 }
"#,
    );

    assert!(matches!(
        validate_parameter_namespaces(&implementation),
        Err(ParameterNamespaceError::UnselectedDefinition {
            namespace: ParameterNamespace::Implementation,
            definition,
            ..
        }) if definition.as_str() == "spare"
    ));
}

#[test]
fn rejects_case_namespaces_for_undefined_selected_definitions() {
    let manifest = parse(
        r#"
version = 1
name = "undefined"

[[experiments]]
name = "main"
problem = "problem"
datasets = ["missing"]
implementations = ["implementation"]
execution_policy = "default"
observation_policy = "final"
implementation_repetitions = 1
measurement_repetitions = 1
seed = 0

[[experiments.cases]]
name = "small"
[experiments.cases.dataset_parameters.missing]
rows = { value = 100 }
"#,
    );

    assert!(matches!(
        validate_parameter_namespaces(&manifest),
        Err(ParameterNamespaceError::UndefinedDefinition {
            location,
            namespace: ParameterNamespace::Dataset,
            definition,
        }) if location.experiment.as_str() == "main"
            && location.case.as_ref().is_some_and(|case| case.as_str() == "small")
            && definition.as_str() == "missing"
    ));
}

#[test]
fn rejects_axes_owned_by_definitions_without_parameter_schemas() {
    let fixed_dataset = parse(
        r#"
version = 1
name = "fixed"

[datasets.fixed]
output_schema = "schemas/dataset.json"
sources = ["datasets/fixed.json"]

[[experiments]]
name = "main"
problem = "problem"
datasets = ["fixed"]
implementations = ["implementation"]
execution_policy = "default"
observation_policy = "final"
implementation_repetitions = 1
measurement_repetitions = 1
seed = 0

[experiments.dataset_parameters.fixed]
fold = { value = 1 }
"#,
    );
    assert!(matches!(
        validate_parameter_namespaces(&fixed_dataset),
        Err(ParameterNamespaceError::DefinitionHasNoParameterSchema {
            namespace: ParameterNamespace::Dataset,
            definition,
            ..
        }) if definition.as_str() == "fixed"
    ));

    let parameterless_generated_dataset = parse(
        r#"
version = 1
name = "parameterless-generated"

[datasets.generated]
output_schema = "schemas/dataset.json"
runner = "command"
program = "generate"
environment = "local"

[[experiments]]
name = "main"
problem = "problem"
datasets = ["generated"]
implementations = ["implementation"]
execution_policy = "default"
observation_policy = "final"
implementation_repetitions = 1
measurement_repetitions = 1
seed = 0

[experiments.dataset_parameters.generated]
fold = { value = 1 }
"#,
    );
    assert!(matches!(
        validate_parameter_namespaces(&parameterless_generated_dataset),
        Err(ParameterNamespaceError::DefinitionHasNoParameterSchema {
            namespace: ParameterNamespace::Dataset,
            definition,
            ..
        }) if definition.as_str() == "generated"
    ));

    let implementation = parse(
        r#"
version = 1
name = "parameterless-implementation"

[implementations.example]
runner = "command"
program = "solve"
environment = "local"
problem_contracts = ["problem"]
capabilities = ["one_shot"]

[[experiments]]
name = "main"
problem = "problem"
implementations = ["example"]
execution_policy = "default"
observation_policy = "final"
implementation_repetitions = 1
measurement_repetitions = 1
seed = 0

[[experiments.cases]]
name = "strict"
[experiments.cases.implementation_parameters.example]
tolerance = { value = 1e-8 }
"#,
    );
    assert!(matches!(
        validate_parameter_namespaces(&implementation),
        Err(ParameterNamespaceError::DefinitionHasNoParameterSchema {
            location,
            namespace: ParameterNamespace::Implementation,
            definition,
        }) if location.experiment.as_str() == "main"
            && location.case.as_ref().is_some_and(|case| case.as_str() == "strict")
            && definition.as_str() == "example"
    ));
}
