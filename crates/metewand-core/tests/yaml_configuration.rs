use std::{fmt::Write as _, path::Path};

use metewand_core::{
    manifest::parse_manifest, manifest_hash::canonical_manifest_bytes,
    problem_contract::parse_problem_contract_document,
};
use serde_json::json;

#[test]
fn yaml_block_flow_alias_and_merge_spellings_have_identical_canonical_bytes() {
    let sources = [
        "version: 1\nname: yaml\nexecution-policies:\n  first: {version: 1, max-concurrency: 2}\n  second: {version: 1, max-concurrency: 3}\n",
        "{version: 1, name: yaml, execution-policies: {first: &base {version: 1, max-concurrency: 2}, second: {<<: *base, max-concurrency: 3}}}",
        "---\nversion: 1\nname: yaml\nexecution-policies:\n  first: &base\n    version: 1\n    max-concurrency: 2\n  second:\n    max-concurrency: 3\n    <<: [*base, {max-concurrency: 4}]\n...\n",
    ];
    let bytes: Vec<_> = sources
        .into_iter()
        .map(|source| {
            canonical_manifest_bytes(&parse_manifest(Path::new("metewand.yaml"), source).unwrap())
                .unwrap()
        })
        .collect();
    assert_eq!(bytes[0], bytes[1]);
    assert_eq!(bytes[0], bytes[2]);
}

#[test]
fn yaml_literal_nulls_reach_canonical_parameters() {
    let source = "version: 1\nname: yaml\nproblems:\n  example:\n    contract: problems/example.yaml\n    parameter-defaults: {missing: null, values: [null, -0.0, 1e0]}\n    evaluator: {runner: command, program: evaluator, sources: [evaluator], environment: local}\n";
    let manifest = parse_manifest(Path::new("metewand.yaml"), source).unwrap();
    assert_eq!(
        manifest.problems["example"]
            .parameter_defaults
            .as_ref()
            .unwrap()
            .as_json(),
        &json!({"missing": null, "values": [null, 0, 1]})
    );
}

#[test]
fn yaml_rejects_explicit_nulls_in_optional_execution_policy_fields() {
    let base = "version: 1\nname: yaml\nexecution-policies:\n  local:\n    version: 1\n";
    assert!(parse_manifest(Path::new("metewand.yaml"), base).is_ok());
    for field in [
        "cpus",
        "threads",
        "memory",
        "network",
        "worker-reuse",
        "warmup-runs",
        "timeout",
        "timing-scope",
        "primary-time",
        "run-order",
        "max-concurrency",
        "enforcement",
    ] {
        for value in ["null", "Null", "NULL", "~", "", "!!null null"] {
            let source = format!("{base}    {field}: {value}\n");
            let error = parse_manifest(Path::new("metewand.yaml"), &source).expect_err(&source);
            assert!(error.message().contains(field), "{error}");
            assert_eq!(error.source_span().unwrap().line, 6, "{field}: {value}");
        }
    }
}

#[test]
fn yaml_rejects_explicit_nulls_in_optional_definition_and_worker_fields() {
    let definitions = [
        (
            "datasets:\n  example:\n    output-schema: schemas/dataset.json\n    runner: command\n    program: materialize\n    environment: local\n",
            &[
                "parameter-schema",
                "parameter-defaults",
                "source",
                "entrypoint",
                "args",
                "sources",
                "protocol-transport",
            ][..],
            4,
        ),
        (
            "datasets:\n  example:\n    output-schema: schemas/dataset.json\n    sources: [datasets/example]\n",
            &["runner"][..],
            4,
        ),
        (
            "implementations:\n  example:\n    runner: command\n    program: solve\n    environment: local\n    problem-contracts: [example]\n    capabilities: [one_shot]\n",
            &[
                "entrypoint",
                "args",
                "sources",
                "protocol-transport",
                "parameter-schema",
                "parameter-defaults",
            ][..],
            4,
        ),
        (
            "problems:\n  example:\n    contract: problem.yaml\n    evaluator: {runner: command, program: evaluate, environment: local}\n",
            &["parameter-defaults"][..],
            4,
        ),
        (
            "problems:\n  example:\n    contract: problem.yaml\n    evaluator:\n      runner: command\n      program: evaluate\n      environment: local\n",
            &["entrypoint", "args", "sources", "protocol-transport"][..],
            6,
        ),
        (
            "problems:\n  example:\n    contract: problem.yaml\n    evaluator:\n      runner: python\n      entrypoint: evaluate.py\n      sources: [evaluate.py]\n      environment: local\n",
            &["program"][..],
            6,
        ),
    ];
    for (definition, fields, indent) in definitions {
        let base = format!("version: 1\nname: yaml\n{definition}");
        assert!(parse_manifest(Path::new("metewand.yaml"), &base).is_ok());
        for field in fields {
            for value in ["null", "~", ""] {
                let source = format!("{base}{:indent$}{field}: {value}\n", "");
                let error = parse_manifest(Path::new("metewand.yaml"), &source).expect_err(&source);
                assert!(error.message().contains(field), "{error}");
                assert_eq!(error.source_span().unwrap().line, source.lines().count());
            }
        }
    }
}

#[test]
fn yaml_rejects_plain_integers_that_overflow_the_yaml_decoders() {
    let values = [
        "0x10000000000000000",
        "+0X10000000000000000",
        "-0x10000000000000000",
        "0x1_0000_0000_0000_0000",
        "0o2000000000000000000000",
        "+0O2000000000000000000000",
        "-0o2000000000000000000000",
        "0b10000000000000000000000000000000000000000000000000000000000000000",
        "+0B10000000000000000000000000000000000000000000000000000000000000000",
        "-0b10000000000000000000000000000000000000000000000000000000000000000",
        "340282366920938463463374607431768211456",
        "+340_282_366_920_938_463_463_374_607_431_768_211_456",
        "-340_282_366_920_938_463_463_374_607_431_768_211_456",
    ];
    for value in values {
        let source = format!("version: 1\nname: {value}\n");
        let error = parse_manifest(Path::new("metewand.yaml"), &source).unwrap_err();
        assert_eq!(error.source_span().unwrap().line, 2, "{value}");

        let source = format!(
            "version: 1\nname: yaml\nproblems: {{example: {{contract: problem.yaml, parameter-defaults: {{value: &overflow {value}, alias: *overflow}}, evaluator: {{runner: command, program: evaluator, environment: local}}}}}}"
        );
        assert!(
            parse_manifest(Path::new("metewand.yaml"), &source).is_err(),
            "{value}"
        );

        let source = format!(
            "version: 1\nname: yaml\nproblems: {{example: {{contract: problem.yaml, parameter-defaults: {{{value}: null}}, evaluator: {{runner: command, program: evaluator, environment: local}}}}}}"
        );
        assert!(
            parse_manifest(Path::new("metewand.yaml"), &source).is_err(),
            "{value}"
        );
    }
}

#[test]
fn yaml_preserves_explicit_numeric_strings_and_non_numeric_plain_strings() {
    let values = [
        "'0x10000000000000000'",
        "\"0x10000000000000000\"",
        "!!str 0x10000000000000000",
        "! 0x10000000000000000",
        "|-\n  0x10000000000000000",
        ">-\n  0x10000000000000000",
        "0x_10000000000000000",
        "0x10000000000000000_",
        "0x10000000__000000000",
        "0o800000000000000000000",
        "0b200000000000000000000",
        "340__282_366_920_938_463_463_374_607_431_768_211_456",
    ];
    for value in values {
        let source = format!("version: 1\nname: {value}\n");
        assert!(
            parse_manifest(Path::new("metewand.yaml"), &source).is_ok(),
            "{value}"
        );
    }
    let source = "version: 1\nname: yaml\nproblems: {example: {contract: problem.yaml, parameter-defaults: {value: &numeric !!str 0x10000000000000000, alias: *numeric}, evaluator: {runner: command, program: evaluator, environment: local}}}";
    assert!(parse_manifest(Path::new("metewand.yaml"), source).is_ok());
}

#[test]
fn yaml_numeric_fallback_errors_use_alias_and_merge_reference_locations() {
    for (values, line) in [
        (
            "      z_anchor: &overflow 0x10000000000000000\n      a_alias: *overflow\n",
            8,
        ),
        (
            "      z_anchor: &overflow {value: 0x10000000000000000}\n      a_merged:\n        <<: *overflow\n",
            9,
        ),
    ] {
        let source = format!(
            "version: 1\nname: yaml\nproblems:\n  example:\n    contract: problem.yaml\n    parameter-defaults:\n{values}    evaluator: {{runner: command, program: evaluator, environment: local}}\n"
        );
        let error = parse_manifest(Path::new("metewand.yaml"), &source).unwrap_err();
        assert_eq!(error.source_span().unwrap().line, line);
    }
}

#[test]
fn yaml_radix_and_separated_safe_integers_remain_numbers() {
    let source = "version: 0b1\nname: yaml\nproblems: {example: {contract: problem.yaml, parameter-defaults: {values: [0x2, 0o2, 0b10, +0X2, -0O2, 9_007_199_254_740_991]}, evaluator: {runner: command, program: evaluator, environment: local}}}";
    let manifest = parse_manifest(Path::new("metewand.yaml"), source).unwrap();
    assert_eq!(
        manifest.problems["example"]
            .parameter_defaults
            .as_ref()
            .unwrap()
            .as_json(),
        &json!({"values": [2, 2, 2, 2, -2, 9_007_199_254_740_991_u64]})
    );
}

#[test]
fn yaml_rejects_invalid_documents_keys_tags_and_json_numbers() {
    let cases = [
        "version = 1\nname = \"legacy\"\n",
        "[version, name]",
        "version: 1\nname: yaml\n---\nversion: 1\nname: other\n",
        "version: 1\nname: yaml\nname: duplicate\n",
        "version: 1\nname: yaml\n1: numeric-key\n",
        "version: 1\nname: yaml\ntrue: boolean-key\n",
        "version: 1\nname: yaml\n? [complex, key]\n: value\n",
        "version: 1\nname: !custom yaml\n",
        "version: 1\nname: !include other.yaml\n",
        "version: 1\nname: yaml\ndatasets: null\n",
        "version: 1\nname: yaml\nexecution-policies: {local: {version: 1, max-concurrency: 9007199254740992}}",
        "version: 1\nname: yaml\nexecution-policies: {local: {version: 1, max-concurrency: .inf}}",
        "version: 1\nname: yaml\nexecution-policies: {local: {version: 1, max-concurrency: .nan}}",
        "version: 1\nname: yaml\nexecution-policies: {local: {version: 1, max-concurrency: 1e999}}",
        "version: 1\nname: yaml\nexecution-policies: {local: {version: 1, max-concurrency: !!binary Mg==}}",
    ];
    for source in cases {
        let error = parse_manifest(Path::new("metewand.yaml"), source).unwrap_err();
        assert!(error.source_span().is_some(), "{source}");
    }
}

#[test]
fn yaml_numeric_and_boolean_keys_are_strings_only_when_quoted() {
    let source =
        "version: 1\nname: yaml\nenvironments: {'1': {kind: local}, 'true': {kind: local}}";
    let manifest = parse_manifest(Path::new("metewand.yaml"), source).unwrap();
    assert_eq!(manifest.environments.len(), 2);
}

#[test]
fn yaml_integer_typed_fields_reject_float_and_string_spellings() {
    for version in ["1.0", "1e0", "'1'"] {
        let source = format!("version: {version}\nname: yaml\n");
        assert!(parse_manifest(Path::new("metewand.yaml"), &source).is_err());
    }
    for value in ["2.0", "2e0", "'2'"] {
        let source = format!(
            "version: 1\nname: yaml\nexecution-policies: {{local: {{version: 1, max-concurrency: {value}}}}}\n"
        );
        assert!(parse_manifest(Path::new("metewand.yaml"), &source).is_err());
    }
    let source = include_str!("../../../fixtures/manifest/v1/complete.yaml");
    for seed in ["2025.0", "2025e0", "'2025'"] {
        let source = source.replace("seed: 2025", &format!("seed: {seed}"));
        let error = parse_manifest(Path::new("metewand.yaml"), &source).unwrap_err();
        assert!(error.message().contains("seed"));
    }
}

#[test]
fn yaml_rejects_duplicate_keys_inside_every_mapping_kind() {
    let sources = [
        "version: 1\nname: yaml\nenvironments: {local: {kind: local}, local: {kind: local}}",
        "version: 1\nname: yaml\nenvironments: {local: {kind: local, kind: local}}",
        "version: 1\nname: yaml\nproblems: {example: {contract: problem.yaml, parameter-defaults: {value: 1, value: 2}, evaluator: {runner: command, program: evaluator, environment: local}}}",
        "version: 1\nname: yaml\nexecution-policies: {first: &base {version: 1, max-concurrency: 1}, second: {<<: *base, max-concurrency: 2, max-concurrency: 3}}",
    ];
    for source in sources {
        let error = parse_manifest(Path::new("metewand.yaml"), source).unwrap_err();
        assert!(error.message().contains("duplicate"), "{}", error.message());
    }
}

#[test]
fn yaml_typed_errors_preserve_unicode_columns_and_alias_reference_locations() {
    let source = "version: 1\nname: yaml\nexecution-policies: {local: {version: 1, memory: café, max-concurrency: zero}}\n";
    let error = parse_manifest(Path::new("metewand.yaml"), source).unwrap_err();
    let span = error.source_span().unwrap();
    assert_eq!(span.line, 3);
    assert_eq!(
        span.column,
        source
            .lines()
            .nth(2)
            .unwrap()
            .split("zero")
            .next()
            .unwrap()
            .chars()
            .count()
            + 1
    );
    assert_eq!(span.end_column - span.column, 4);

    let source = "version: 1\nname: yaml\nexecution-policies:\n  first: {version: 1, memory: &bad zero}\n  second: {version: 1, max-concurrency: *bad}\n";
    let error = parse_manifest(Path::new("metewand.yaml"), source).unwrap_err();
    let span = error.source_span().unwrap();
    assert_eq!((span.line, span.column), (5, 41));
}

#[test]
fn yaml_alias_expansion_remains_bounded() {
    let source = "version: 1\nname: yaml\ndatasets: &recursive {<<: *recursive}\n";
    assert!(parse_manifest(Path::new("metewand.yaml"), source).is_err());

    let mut source = String::from(
        "version: 1\nname: yaml\nproblems:\n  example:\n    contract: problem.yaml\n    parameter-defaults:\n      level0: &level0 null\n",
    );
    for level in 1..70 {
        writeln!(
            source,
            "      level{level}: &level{level} [*level{}]",
            level - 1
        )
        .unwrap();
    }
    source.push_str("    evaluator: {runner: command, program: evaluator, environment: local}\n");
    let error = parse_manifest(Path::new("metewand.yaml"), &source).unwrap_err();
    assert!(
        error.message().contains("budget breached: Depth"),
        "{}",
        error.message()
    );
    assert!(error.source_span().is_some());
}

#[test]
fn yaml_nesting_budget_accepts_the_boundary_and_rejects_deeper_values() {
    for (array_depth, accepted) in [(20, true), (21, false)] {
        let source = format!(
            "version: 1\nname: yaml\nproblems:\n  example:\n    contract: problem.yaml\n    parameter-defaults: {{value: {}null{}}}\n    evaluator: {{runner: command, program: evaluator, environment: local}}\n",
            "[".repeat(array_depth),
            "]".repeat(array_depth),
        );
        let result = parse_manifest(Path::new("metewand.yaml"), &source);
        assert_eq!(result.is_ok(), accepted, "{result:?}");
        if let Err(error) = result {
            assert!(error.message().contains("budget breached: Depth"));
            assert_eq!(error.source_span().unwrap().line, 6);
        }
    }
}

#[test]
fn yaml_accepts_json_core_tags_and_rejects_binary_and_timestamp_tags() {
    let source =
        "!!map {version: !!int 1, name: !!str yaml, environments: !!map {local: {kind: local}}}";
    assert!(parse_manifest(Path::new("metewand.yaml"), source).is_ok());
    for value in ["!!binary Mg==", "!!timestamp 2026-10-23", "!int 1"] {
        let source = format!(
            "version: 1\nname: yaml\nproblems:\n  example:\n    contract: problems/example.yaml\n    parameter-defaults: {{value: {value}}}\n    evaluator: {{runner: command, program: evaluator, environment: local}}\n"
        );
        let error = parse_manifest(Path::new("metewand.yaml"), &source).unwrap_err();
        assert!(!error.message().is_empty(), "{value}");
        assert_eq!(error.source_span().unwrap().line, 6, "{value}");
    }
}

#[test]
fn problem_contract_uses_the_same_yaml_document_rules() {
    let source = "version: 1\nname: yaml\nfamily: optimization\nparameter-schema: schemas/parameters.json\ndataset-schemas: []\nresult-schema: schemas/result.json\nmetric-schema: schemas/metrics.json\nsemantics-schema: schemas/semantics.json\nallowed-timing-scopes: [execute_only]\nsupported-budgets: [none]\nsemantics: {preprocessing: {}, validity: {nonfinite: reject, missing: null}, one_shot_completion: {maximum_error: 0.001}}\nreference-cases:\n  - result: fixtures/result.json\n    expected-metrics: fixtures/metrics.json\n";
    let contract = parse_problem_contract_document(Path::new("problem.yaml"), source).unwrap();
    assert_eq!(contract.name.as_str(), "yaml");
    for value in ["null", "~", ""] {
        let source = format!("{source}    dataset: {value}\n");
        let error =
            parse_problem_contract_document(Path::new("problem.yaml"), &source).expect_err(&source);
        assert!(error.message().unwrap().contains("dataset"), "{error}");
        assert_eq!(error.source_span().unwrap().line, source.lines().count());
    }
    assert!(
        parse_problem_contract_document(
            Path::new("problem.yaml"),
            &source.replace("family: optimization", "family: !include missing.yaml")
        )
        .is_err()
    );
}

#[test]
fn kebab_case_options_preserve_user_keys_and_canonical_record_spelling() {
    let source = "version: 1\nname: kebab\nenvironments: {local: {kind: local}}\nproblems:\n  user_problem:\n    contract: problem.yaml\n    parameter-defaults: {user_key: 1}\n    evaluator: {runner: command, program: evaluator, environment: local}\nexecution-policies:\n  user_policy: {version: 1, max-concurrency: 2}\n";
    let manifest = parse_manifest(Path::new("metewand.yaml"), source).unwrap();
    assert_eq!(
        manifest.problems["user_problem"]
            .parameter_defaults
            .as_ref()
            .unwrap()
            .as_json(),
        &json!({"user_key": 1})
    );
    let canonical: serde_json::Value =
        serde_json::from_slice(&canonical_manifest_bytes(&manifest).unwrap()).unwrap();
    assert_eq!(
        canonical["execution_policies"]["user_policy"]["max_concurrency"],
        2
    );
    for (key, old_key) in [
        ("execution-policies", "execution_policies"),
        ("parameter-defaults", "parameter_defaults"),
        ("max-concurrency", "max_concurrency"),
    ] {
        let error = parse_manifest(Path::new("metewand.yaml"), &source.replace(key, old_key))
            .expect_err(old_key);
        assert!(error.message().contains(old_key), "{error}");
    }
}

#[test]
fn problem_contracts_reject_underscore_option_keys() {
    let source = include_str!("../../../fixtures/problem-contract/v1/complete.yaml");
    parse_problem_contract_document(Path::new("problem.yaml"), source).unwrap();
    for (key, old_key) in [
        ("parameter-schema", "parameter_schema"),
        ("allowed-timing-scopes", "allowed_timing_scopes"),
        ("expected-metrics", "expected_metrics"),
    ] {
        let error = parse_problem_contract_document(
            Path::new("problem.yaml"),
            &source.replace(key, old_key),
        )
        .expect_err(old_key);
        assert!(error.to_string().contains(old_key), "{error}");
    }
}
